//! The vector index's behaviour through the fakes: projector, jobs, migration and search.
use memex_core::testing::{FakeEmbedder, ManualClock, MemBlobStore, MemRegistry, MemStore, MemVector, SeededRandom, mem_search, vector_contract};
use memex_core::vector::{EMBED_BATCH, EmbedDocRef, EmbedInput, MissingEmbedding, content_hash, embed_requests, embedded_by, embedding_text};
use memex_core::{Caller, EMBED_JOB, Embedder, Server, ServerEdges, ServerError, Snapshot, StepOutcome, Store, VectorIndex};
use memex_types::{
    DocKind, EmbedError, EmbedPurpose, EventDraft, Generation, Header, IndexDoc, JobState, LexicalFilter, Mutation, ObjectId, Row, RowKey, SegmentRow,
    SourceRow, Status, Table, VaultId, VectorHealth,
};
use std::collections::BTreeSet;

struct Fakes;

impl ServerEdges for Fakes {
    type Registry = MemRegistry;
    type Store = MemStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = ManualClock;
    type Seed = SeededRandom;
    type Rasterizer = memex_core::testing::FakeRasterizer;
    type Lexical = memex_core::testing::MemLexical;
    type Vector = MemVector;
    type Embedder = FakeEmbedder;
    type Reranker = memex_core::testing::FakeReranker;
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

fn header(id: &str, status: Status) -> Header {
    Header {
        id: ObjectId(id.into()),
        creator: "t".into(),
        status,
        version: 1,
        acl_id: None,
    }
}

fn source(id: &str, name: &str) -> Row {
    Row::Source(SourceRow {
        header: header(id, Status::Active),
        sha256: format!("sha-{id}"),
        media_type: "text/plain".into(),
        trust_tier: "unverified".into(),
        recorded_at_ms: 0,
        name: name.into(),
        size: 1,
        logical_id: ObjectId(id.into()),
        version_no: 1,
        metadata: serde_json::json!({}),
    })
}

fn segment(id: &str, text: &str, status: Status) -> SegmentRow {
    let mut row: SegmentRow = serde_json::from_value(serde_json::json!({
        "header": header(id, status),
        "source_id": "src",
        "language": null,
        "text": text,
    }))
    .unwrap();
    row.heading = None;
    row
}

impl World {
    fn record(&self, mutations: Vec<Mutation>) {
        let lease = self.s.vault_lease_for_tests(&self.v).unwrap();
        let payload = serde_json::to_value(memex_core::StateRecorded { mutations }).unwrap();
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
            .unwrap();
    }

    fn put_segments(&self, rows: &[(&str, &str)]) {
        self.record(
            rows.iter()
                .map(|(id, text)| Mutation::Put(Row::Segment(segment(id, text, Status::Active))))
                .collect(),
        );
    }

    fn project(&self) -> usize {
        let mut steps = 0;
        while self.s.project_vector(&self.v).unwrap() == StepOutcome::Progress {
            steps += 1;
            assert!(steps < 10_000, "the projector never settles");
        }
        steps
    }

    fn run_embed_jobs(&self) -> usize {
        let mut ran = 0;
        while let Some(job) = self.s.claim_job(&self.v, EMBED_JOB, "w").unwrap() {
            match self.s.run_embed_job(&self.v, &job) {
                Ok(_) => assert!(self.s.complete_job(&self.v, &job, vec![]).unwrap()),
                Err(e) => assert!(self.s.fail_job(&self.v, &job, &e.message, e.retryable).unwrap()),
            }
            ran += 1;
        }
        ran
    }

    /// Everything settled: indexed, embedded and announced.
    fn settle(&self) {
        for _ in 0..50 {
            self.project();
            if self.run_embed_jobs() == 0 {
                self.project();
                return;
            }
        }
        panic!("work never finished");
    }

    fn jobs(&self, state: JobState) -> usize {
        let snap = self.s.engine().store().snapshot(&self.v).unwrap();
        snap.jobs(Some(EMBED_JOB), Some(state), None, 100_000).unwrap().len()
    }

    fn find(&self, text: &str) -> Vec<String> {
        self.s
            .search_vector(&self.c, &self.v, text, LexicalFilter::default(), 50)
            .unwrap()
            .hits
            .into_iter()
            .map(|h| h.id.0)
            .collect()
    }

    fn embedder(&self) -> &FakeEmbedder {
        self.s.embedder()
    }
}

fn set(ids: &[String]) -> BTreeSet<String> {
    ids.iter().cloned().collect()
}

#[test]
fn the_contract_passes_on_the_fake() {
    vector_contract::run_all(&MemVector::new(), "c-", &|_| {});
}

#[test]
fn new_content_gets_embedded_by_a_job_and_becomes_searchable() {
    let w = world();
    w.put_segments(&[("s1", "first text"), ("s2", "second text")]);
    assert!(matches!(
        w.s.search_vector(&w.c, &w.v, "q", LexicalFilter::default(), 5),
        Err(ServerError::Unavailable(_))
    ));
    w.settle();
    assert_eq!(w.embedder().passages_embedded(), 2);
    assert_eq!(set(&w.find("anything")), set(&["s1".into(), "s2".into()]));
    assert_eq!(w.embedder().queries_embedded(), 1);
    assert_eq!(w.jobs(JobState::Done), 1, "two texts fit one job");
}

#[test]
fn a_query_is_embedded_as_a_query_and_finds_its_own_text() {
    let w = world();
    w.put_segments(&[("s1", "alpha"), ("s2", "beta"), ("s3", "gamma")]);
    w.settle();
    // The fake's vectors depend only on the exact text, so a hit on identical text scores 1.
    let hits = w.s.search_vector(&w.c, &w.v, "beta", LexicalFilter::default(), 1).unwrap().hits;
    assert_eq!(hits[0].id.0, "s2");
    assert!((hits[0].score - 1.0).abs() < 1e-4);
}

#[test]
fn identical_text_embeds_once() {
    let w = world();
    w.put_segments(&[("a", "same words"), ("b", "same words"), ("c", "other words")]);
    w.settle();
    assert_eq!(w.embedder().passages_embedded(), 2);
    assert_eq!(w.s.vector().stored_embeddings(&w.v), 2);
    w.put_segments(&[("d", "same words")]);
    w.settle();
    assert_eq!(w.embedder().passages_embedded(), 2, "a later copy needs no job's embedding");
    assert_eq!(w.find("same words").len(), 4);
}

#[test]
fn a_rebuild_makes_no_embedder_calls() {
    let w = world();
    let rows: Vec<(String, String)> = (0..200).map(|i| (format!("s{i:03}"), format!("text number {i}"))).collect();
    let refs: Vec<(&str, &str)> = rows.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    w.put_segments(&refs);
    w.settle();
    let embedded = w.embedder().passages_embedded();
    assert_eq!(embedded, 200);
    let before = set(&w.find("q"));
    w.s.rebuild_vector(&w.v);
    w.settle();
    assert_eq!(w.embedder().passages_embedded(), embedded, "nothing was embedded again");
    assert_eq!(set(&w.find("q")), before);
}

#[test]
fn a_big_vault_is_split_into_batches() {
    let w = world();
    let rows: Vec<(String, String)> = (0..(EMBED_BATCH * 2 + 1)).map(|i| (format!("s{i}"), format!("t{i}"))).collect();
    let refs: Vec<(&str, &str)> = rows.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    w.put_segments(&refs);
    w.project();
    assert_eq!(w.jobs(JobState::Queued), 3);
}

#[test]
fn repeating_a_step_queues_nothing_twice() {
    let w = world();
    w.put_segments(&[("s1", "text")]);
    w.project();
    assert_eq!(w.jobs(JobState::Queued), 1);
    w.s.rebuild_vector(&w.v);
    w.project();
    assert_eq!(w.jobs(JobState::Queued), 1, "the rebuild found the same work already queued");
}

#[test]
fn a_failed_embedding_retries_that_job_only() {
    let w = world();
    let rows: Vec<(String, String)> = (0..(EMBED_BATCH + 1)).map(|i| (format!("s{i}"), format!("t{i}"))).collect();
    let refs: Vec<(&str, &str)> = rows.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    w.put_segments(&refs);
    w.project();
    assert_eq!(w.jobs(JobState::Queued), 2);
    w.embedder().fail_next(1);
    let ran = w.run_embed_jobs();
    assert_eq!(ran, 2, "the failed job waits for its delay, and the other one runs");
    assert_eq!(w.jobs(JobState::Done), 1);
    assert_eq!(w.jobs(JobState::Queued), 1, "the failed one is back in the queue");
    w.project();
    let found = w.find("q").len();
    assert!(found == 1 || found == EMBED_BATCH, "what finished is already searchable, and got {found}");
    w.s.engine().clock().advance(std::time::Duration::from_secs(600));
    let embedded = w.embedder().passages_embedded();
    w.settle();
    assert_eq!(w.find("q").len(), 50.min(EMBED_BATCH + 1));
    assert!(
        w.embedder().passages_embedded() - embedded <= EMBED_BATCH as u64,
        "the finished job's texts weren't embedded again"
    );
}

#[test]
fn a_model_returning_the_wrong_dimension_goes_dead_at_once() {
    let w = world();
    w.put_segments(&[("s1", "text")]);
    w.project();
    w.embedder().return_wrong_dimension();
    w.run_embed_jobs();
    assert_eq!(w.jobs(JobState::Dead), 1, "another attempt couldn't help");
    assert_eq!(w.s.vector().stored_embeddings(&w.v), 0, "a bad vector is never stored");
}

#[test]
fn a_changed_or_retired_segment_leaves_the_index() {
    let w = world();
    w.put_segments(&[("s1", "old text"), ("s2", "kept")]);
    w.settle();
    let mut retired = segment("s2", "kept", Status::Superseded);
    retired.header.version = 2;
    let mut changed = segment("s1", "new text", Status::Active);
    changed.header.version = 2;
    w.record(vec![Mutation::Put(Row::Segment(retired)), Mutation::Put(Row::Segment(changed))]);
    w.project();
    assert!(w.find("q").is_empty(), "s1's old vector mustn't answer for its new text, and s2 is retracted");
    w.settle();
    assert_eq!(w.find("q"), ["s1"]);
    let hits = w.s.search_vector(&w.c, &w.v, "new text", LexicalFilter::default(), 1).unwrap().hits;
    assert!((hits[0].score - 1.0).abs() < 1e-4, "it's the vector of the new text");
}

#[test]
fn a_hit_retired_since_it_was_indexed_is_fenced_out() {
    let w = world();
    w.put_segments(&[("s1", "text")]);
    w.settle();
    let mut retired = segment("s1", "text", Status::Superseded);
    retired.header.version = 2;
    w.record(vec![Mutation::Put(Row::Segment(retired))]);
    // The projector hasn't run, so the index still holds it.
    assert!(w.find("q").is_empty());
}

#[test]
fn the_document_title_is_part_of_what_gets_embedded() {
    let doc = IndexDoc {
        id: ObjectId("s".into()),
        kind: DocKind::Segment,
        language: None,
        text: "body".into(),
        source_ids: vec![],
        epistemic: None,
        valid_from_ms: None,
        valid_to_ms: None,
        doc_type: None,
        trust_tiers: vec![],
        attrs: vec![],
    };
    assert_eq!(embedding_text(&doc, Some("Manual")), "Manual\nbody");
    assert_eq!(embedding_text(&doc, Some("")), "body");
    assert_eq!(embedding_text(&doc, None), "body");
    assert_ne!(content_hash("Manual\nbody"), content_hash("body"));

    let w = world();
    w.record(vec![Mutation::Put(source("src", "Manual"))]);
    w.put_segments(&[("s1", "body")]);
    w.settle();
    let hits = w.s.search_vector(&w.c, &w.v, "Manual\nbody", LexicalFilter::default(), 1).unwrap().hits;
    assert!((hits[0].score - 1.0).abs() < 1e-4, "the same text with its title scores 1");
}

#[test]
fn a_new_model_is_built_side_by_side_and_queries_never_mix() {
    let w = world();
    w.put_segments(&[("s1", "one"), ("s2", "two")]);
    w.settle();
    assert_eq!(w.find("one")[0], "s1");

    // A restart with a new active model: the projector's state is gone, and only what's durable counts.
    w.embedder().activate("fake-b", 5);
    w.s.reset_vector_projector_for_tests();
    w.project();
    assert_eq!(w.s.vector().building_model(&w.v).map(|m| m.name), Some("fake-b".to_string()));
    assert!(matches!(w.s.vector().health(&w.v).unwrap(), VectorHealth::Ready { model, .. } if model.name == "fake-a"));
    // It can't flip yet, since the new model has no vectors, and the old one keeps answering with old-model queries.
    let queries = w.embedder().queries_embedded();
    assert_eq!(w.find("two")[0], "s2");
    assert_eq!(w.embedder().queries_embedded(), queries + 1);
    assert!(
        matches!(w.s.vector().health(&w.v).unwrap(), VectorHealth::Ready { model, .. } if model.name == "fake-a"),
        "still the old one"
    );

    // Content arriving during the move reaches both generations.
    w.put_segments(&[("s3", "three")]);
    w.settle();
    assert!(
        matches!(w.s.vector().health(&w.v).unwrap(), VectorHealth::Ready { model, .. } if model.dimension == 5),
        "flipped once complete"
    );
    assert_eq!(set(&w.find("q")), set(&["s1".into(), "s2".into(), "s3".into()]));
    assert_eq!(w.find("three")[0], "s3", "a new-model query finds new-model vectors");
}

#[test]
fn a_move_waits_for_a_dead_job_rather_than_lose_results() {
    let w = world();
    w.put_segments(&[("s1", "one")]);
    w.settle();
    w.embedder().activate("fake-b", 5);
    w.s.reset_vector_projector_for_tests();
    w.project();
    w.embedder().return_wrong_dimension();
    w.run_embed_jobs();
    assert_eq!(w.jobs(JobState::Dead), 1);
    w.project();
    assert!(matches!(w.s.vector().health(&w.v).unwrap(), VectorHealth::Ready { model, .. } if model.name == "fake-a"));
    assert_eq!(w.find("one"), ["s1"], "search still answers");
}

#[test]
fn startup_refuses_a_missing_model_or_a_changed_dimension() {
    let w = world();
    w.put_segments(&[("s1", "one")]);
    w.settle();
    w.s.check_embedding_models().unwrap();
    w.embedder().activate("fake-b", 5);
    w.s.check_embedding_models().unwrap();
    w.embedder().remove("fake-a");
    let e = w.s.check_embedding_models().unwrap_err();
    assert!(e.to_string().contains("fake-a"), "{e}");
    w.embedder().activate("fake-a", 9);
    let e = w.s.check_embedding_models().unwrap_err();
    assert!(e.to_string().contains("dimensions"), "{e}");
}

#[test]
fn an_embedder_can_t_be_asked_for_a_model_it_lacks() {
    let e = FakeEmbedder::default();
    assert_eq!(
        e.embed("nope", EmbedPurpose::Query, &["x".into()]),
        Err(EmbedError::UnknownModel("nope".into()))
    );
}

#[test]
fn a_failed_apply_retries_from_durable_state_and_loses_nothing() {
    let w = world();
    w.put_segments(&[("s1", "one")]);
    w.settle();
    w.put_segments(&[("s2", "two")]);
    w.s.vector().fail_next_applies(1);
    assert!(w.s.project_vector(&w.v).is_err());
    assert!(w.s.vector().watermark(&w.v, Generation::Live).unwrap() < w.s.engine().store().snapshot(&w.v).unwrap().head().unwrap());
    w.settle();
    assert_eq!(set(&w.find("q")), set(&["s1".into(), "s2".into()]));
}

#[test]
fn deleted_rows_leave_the_index() {
    let w = world();
    w.put_segments(&[("s1", "one"), ("s2", "two")]);
    w.settle();
    w.record(vec![Mutation::Delete(RowKey {
        table: Table::Segment,
        id: ObjectId("s1".into()),
    })]);
    w.settle();
    assert_eq!(w.find("q"), ["s2"]);
}

#[test]
fn bad_query_text_is_refused_and_other_vaults_stay_apart() {
    let w = world();
    w.put_segments(&[("s1", "one")]);
    w.settle();
    assert!(matches!(
        w.s.search_vector(&w.c, &w.v, "   ", LexicalFilter::default(), 5),
        Err(ServerError::Invalid(_))
    ));
    let long = "x".repeat(5000);
    assert!(matches!(
        w.s.search_vector(&w.c, &w.v, &long, LexicalFilter::default(), 5),
        Err(ServerError::Invalid(_))
    ));
    let other = w.s.create_vault(&w.c, "other").unwrap().id;
    assert!(matches!(
        w.s.search_vector(&w.c, &other, "q", LexicalFilter::default(), 5),
        Err(ServerError::Unavailable(_))
    ));
}

#[test]
fn a_job_key_follows_the_current_text() {
    let doc = |id: &str| EmbedDocRef {
        kind: DocKind::Segment,
        id: ObjectId(id.into()),
    };
    let missing = |hash: &str| MissingEmbedding {
        doc: doc("a"),
        hash: hash.into(),
    };
    let first = embed_requests("m", &[missing("h1")]);
    assert_eq!(first, embed_requests("m", &[missing("h1")]));
    assert_ne!(first[0].dedupe_key, embed_requests("m", &[missing("h2")])[0].dedupe_key);
    assert_ne!(first[0].dedupe_key, embed_requests("other", &[missing("h1")])[0].dedupe_key);
}

#[test]
fn only_a_finished_embed_job_announces_anything() {
    let input = serde_json::to_value(EmbedInput {
        model: "m".into(),
        docs: vec![EmbedDocRef {
            kind: DocKind::Claim,
            id: ObjectId("c".into()),
        }],
    })
    .unwrap();
    let mut job = memex_core::jobs::new_job(ObjectId("j".into()), "t", EMBED_JOB, input, "k".into(), 1, 0);
    assert!(embedded_by(&job).is_empty(), "queued");
    job.state = JobState::Done;
    assert_eq!(embedded_by(&job).len(), 1);
    job.kind = "rendition".into();
    assert!(embedded_by(&job).is_empty(), "another kind");
}
