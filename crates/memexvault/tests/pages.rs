//! Wiki pages on real Postgres, tantivy and pgvector. Run with `MEMEX_TEST_DATABASE_URL` set, as in AGENTS.md. Timings are taken on a machine shared with other work.
// These tests read the environment, measure time and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::testing::{FakeEmbedder, FakeRasterizer, FakeReranker, ManualClock, MemBlobStore, MemRegistry};
use memex_core::{ClaimRefStatus, NewPage, Proposal, SearchEdges, Server, ServerEdges, ServerError, Snapshot, Store, dump_store};
use memex_lexical::TantivyLexical;
use memex_store_postgres::PgStore;
use memex_types::{DocKind, Epistemic, ItemKind, ObjectId, Op, RiskDecision, SearchMode, SearchRequest, VaultId};
use memex_vector::PgVectors;
use memexvault::edges::{PgVectorIndex, SystemClock, SystemSeed, TantivyIndex};
use memexvault::pg_edge::PostgresStore;
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

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

fn rig(name: &str) -> Rig {
    let url = std::env::var("MEMEX_TEST_DATABASE_URL").expect("not run: MEMEX_TEST_DATABASE_URL isn't set");
    let pg = PgStore::connect(&url, 16).expect("connect");
    pg.migrate().expect("migrate");
    let pg = Arc::new(pg);
    let dir = std::env::temp_dir().join(format!("memex-pages-{name}-{}", std::process::id()));
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
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, name).unwrap().id;
    Rig { s, admin, v, dir }
}

impl Rig {
    fn create(&self, title: &str, body: &str) -> ObjectId {
        let new = NewPage {
            title: title.into(),
            slug: None,
            body: body.into(),
            rationale: "r".into(),
            idempotency_key: format!("create-{title}"),
        };
        let (id, out) = self.s.propose_new_page(&self.admin, &self.v, new, false).unwrap();
        assert_eq!(out.risk, RiskDecision::AutoCommit, "{:?}", out.validators);
        id
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p) as usize]
}

#[test]
#[ignore = "needs Postgres"]
fn pg_page_new_pages_are_found_within_the_lag_target() {
    let r = rig("pg-page-lag");
    let stop = Arc::new(AtomicBool::new(false));
    let threads = [
        memexvault::workers::start_projector(r.s.clone(), stop.clone()),
        memexvault::workers::start_vector_projector(r.s.clone(), stop.clone()),
    ];
    let deadline = Instant::now() + Duration::from_secs(30);
    while r.s.search_lexical(&r.admin, &r.v, "nothing", Default::default(), 1).is_err() {
        assert!(Instant::now() < deadline, "the keyword index never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut lags = vec![];
    for i in 0..40 {
        let word = format!("quixotic{i}");
        let started = Instant::now();
        let id = r.create(&format!("Lag page {i}"), &format!("# Part\n\nA body with {word} in it.\n"));
        let block = r.s.get_wiki_page(&r.admin, &r.v, &id, None).unwrap().content.blocks[1].id.clone();
        loop {
            let found = r.s.search_lexical(&r.admin, &r.v, &word, Default::default(), 5).unwrap();
            if found.hits.iter().any(|h| h.id == block && h.kind == DocKind::Block) {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(30), "page {i} never reached the keyword index");
            std::thread::sleep(Duration::from_millis(5));
        }
        lags.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    lags.sort_by(f64::total_cmp);
    let p95 = percentile(&lags, 0.95);
    println!(
        "commit to keyword hit (busy machine): p50 {:.0} ms, p95 {p95:.0} ms, max {:.0} ms, n={}",
        percentile(&lags, 0.5),
        lags[lags.len() - 1],
        lags.len()
    );
    assert!(p95 <= 1000.0, "lag p95 {p95} ms is over the 1 s target");
    // It reaches the vector index once its embed job runs, as with any new text.
    let started = Instant::now();
    let mut vector = SearchRequest::new("quixotic7");
    vector.mode = SearchMode::Vector;
    vector.filter.kinds = vec![DocKind::Block];
    loop {
        while let Some(job) = r.s.claim_job(&r.v, memex_core::EMBED_JOB, "t").unwrap() {
            r.s.run_embed_job(&r.v, &job).unwrap();
            assert!(r.s.complete_job(&r.v, &job, vec![]).unwrap());
        }
        if r.s.search(&r.admin, &r.v, &vector).unwrap().items.iter().any(|i| i.kind == ItemKind::Block) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "no block reached the vector index");
        std::thread::sleep(Duration::from_millis(20));
    }
    println!(
        "embedding and vector index took {} ms for 40 pages (busy machine)",
        started.elapsed().as_millis()
    );
    let mut hybrid = SearchRequest::new("quixotic7");
    hybrid.filter.kinds = vec![DocKind::Block, DocKind::Page];
    let p = r.s.search(&r.admin, &r.v, &hybrid).unwrap();
    assert!(p.degraded.is_empty(), "{:?}", p.degraded);
    assert_eq!(p.items[0].kind, ItemKind::Block);
    stop.store(true, Ordering::Relaxed);
    for t in threads {
        t.join().unwrap();
    }
}

#[test]
#[ignore = "needs Postgres"]
fn pg_page_no_update_is_lost_under_100_concurrent_editors() {
    let r = rig("pg-page-editors");
    let id = r.create("Contested", "start");
    let started = Instant::now();
    // Each read takes a connection, and this server is shared, so at most 16 editors read at a time. Writes queue on the vault's lock already, which is what's under test.
    let readers = Arc::new((Mutex::new(0usize), Condvar::new()));
    let threads: Vec<_> = (0..100)
        .map(|i| {
            let (s, c, v, id, readers) = (r.s.clone(), r.admin.clone(), r.v.clone(), id.clone(), readers.clone());
            std::thread::spawn(move || {
                let mut retries = 0u32;
                loop {
                    let page = {
                        let (count, freed) = &*readers;
                        let mut n = freed.wait_while(count.lock().unwrap(), |n| *n >= 16).unwrap();
                        *n += 1;
                        drop(n);
                        let page = s.get_wiki_page(&c, &v, &id, None).unwrap();
                        *count.lock().unwrap() -= 1;
                        freed.notify_one();
                        page
                    };
                    let body = format!("{}\nline {i}\n", page.content.body.trim_end());
                    let key = format!("editor-{i}-{}", page.current_revision);
                    match s.propose_page_edit(&c, &v, &id, page.current_revision, body, "r".into(), key, false) {
                        Ok(_) => return retries,
                        Err(ServerError::StaleVersion { .. }) => retries += 1,
                        Err(e) => panic!("{e}"),
                    }
                }
            })
        })
        .collect();
    let retries: u32 = threads.into_iter().map(|t| t.join().unwrap()).sum();
    let page = r.s.get_wiki_page(&r.admin, &r.v, &id, None).unwrap();
    assert_eq!(page.current_revision, 101);
    for i in 0..100 {
        assert!(page.content.body.contains(&format!("line {i}\n")), "editor {i}'s line is gone");
    }
    println!(
        "100 editors, every update kept, {retries} stale retries, {} ms (busy machine)",
        started.elapsed().as_millis()
    );
    // Without retrying, exactly one of 100 gets through from the same version.
    let id = r.create("Raced", "start");
    let threads: Vec<_> = (0..100)
        .map(|i| {
            let (s, c, v, id) = (r.s.clone(), r.admin.clone(), r.v.clone(), id.clone());
            std::thread::spawn(move || s.propose_page_edit(&c, &v, &id, 1, format!("from {i}"), "r".into(), format!("race-{i}"), false))
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|x| x.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|x| matches!(x, Err(ServerError::StaleVersion { .. }))).count(), 99);
}

#[test]
#[ignore = "needs Postgres"]
fn pg_page_rename_keeps_links_on_a_generated_graph_of_10k_pages() {
    let r = rig("pg-page-graph");
    const PAGES: usize = 10_000;
    let title = |i: usize| format!("Graph page {i}");
    let hub = r.create("Hub", "the hub");
    let ids: Vec<ObjectId> = (0..PAGES).map(|i| ObjectId(format!("page{i:05}"))).collect();
    let started = Instant::now();
    for chunk in (0..PAGES).collect::<Vec<_>>().chunks(500) {
        let ops: Vec<Op> = chunk
            .iter()
            .map(|i| Op::CreatePage {
                id: ids[*i].clone(),
                title: title(*i),
                body: format!(
                    "Links to [[{}]] and [[{}]]{}.",
                    title((i + 1) % PAGES),
                    title((i + 2) % PAGES),
                    if i % 10 == 0 { " and [[Hub]]" } else { "" }
                ),
                slug: None,
                redirects: vec![],
            })
            .collect();
        let proposal = Proposal {
            ops,
            rationale: "graph".into(),
            idempotency_key: format!("graph-{}", chunk[0]),
        };
        assert_eq!(r.s.propose(&r.admin, &r.v, proposal, false).unwrap().risk, RiskDecision::AutoCommit);
    }
    println!("10k linked pages written in {:.1} s (busy machine)", started.elapsed().as_secs_f64());
    let backlinks = |id: &ObjectId| {
        let mut all = vec![];
        let mut after = None;
        loop {
            let page = r.s.page_backlinks(&r.admin, &r.v, id, after.as_ref(), 200).unwrap();
            let Some(last) = page.last() else { break };
            after = Some(last.edge_id.clone());
            all.extend(page.into_iter().map(|b| b.from_page));
        }
        all.sort();
        all
    };
    let hub_before = backlinks(&hub);
    assert_eq!(hub_before.len(), 1000);
    assert_eq!(backlinks(&ids[5000]), vec![ids[4998].clone(), ids[4999].clone()]);
    assert!(r.s.unresolved_page_links(&r.admin, &r.v, None, 10).unwrap().is_empty());

    let head = r.s.engine().store().snapshot(&r.v).unwrap().head().unwrap();
    let t = Instant::now();
    r.s.propose_page_rename(&r.admin, &r.v, &hub, 1, "The Central Hub".into(), None, "r".into(), "h".into())
        .unwrap();
    println!(
        "renaming a page with 1000 backlinks took {:.1} ms (busy machine)",
        t.elapsed().as_secs_f64() * 1000.0
    );
    r.s.propose_page_rename(&r.admin, &r.v, &ids[5000], 1, "Renamed".into(), None, "r".into(), "m".into())
        .unwrap();
    let snap = r.s.engine().store().snapshot(&r.v).unwrap();
    assert_eq!(snap.events(head).unwrap().len(), 2, "one event each, whatever links to them");
    drop(snap);
    assert_eq!(backlinks(&hub), hub_before);
    assert_eq!(backlinks(&ids[5000]), vec![ids[4998].clone(), ids[4999].clone()]);
    assert!(r.s.page_links(&r.admin, &r.v, &ids[4999], None, 10).unwrap().iter().all(|l| l.resolved));
    assert!(r.s.unresolved_page_links(&r.admin, &r.v, None, 10).unwrap().is_empty());
    let old = r.s.page_redirect(&r.admin, &r.v, "graph-page-5000").unwrap();
    assert!(old.redirected && old.current_slug == "renamed" && old.page_id == ids[5000]);
}

#[test]
#[ignore = "needs Postgres"]
fn pg_page_revert_claims_delete_and_import_work_on_the_real_store() {
    let r = rig("pg-page-life");
    r.s.record_segment(
        &r.admin,
        &r.v,
        &ObjectId("seg".into()),
        &ObjectId("src".into()),
        None,
        "The pump weighs 12 kg, or 13 kg by now.",
    )
    .unwrap();
    let claim = |id: &str, value: serde_json::Value, quote: &str| Op::AddAssertion {
        id: ObjectId(id.into()),
        subject: "pump".into(),
        predicate: "weighs".into(),
        value,
        epistemic: Epistemic::Asserted,
        machine: false,
        confidence_permille: 1000,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: vec![(
            ObjectId(format!("{id}-e")),
            memex_types::EvidenceInput {
                segment_id: ObjectId("seg".into()),
                quote: quote.into(),
                stance: memex_types::Stance::Supports,
            },
        )],
    };
    let propose = |key: &str, ops: Vec<Op>| {
        r.s.propose(
            &r.admin,
            &r.v,
            Proposal {
                ops,
                rationale: "r".into(),
                idempotency_key: key.into(),
            },
            false,
        )
        .unwrap()
    };
    propose("c1", vec![claim("old", json!(12), "weighs 12 kg")]);
    let first = "# Pump\n\nWeight {{claim:old}}, see [[Other]].\n\n```\ncode\n\nmore\n```\n";
    let id = r.create("Pump Page", first);
    let other = r.create("Other", "Back to [[Pump Page]], and [[Nowhere]].");
    let v1 = r.s.get_wiki_page(&r.admin, &r.v, &id, None).unwrap();
    r.s.propose_page_edit(&r.admin, &r.v, &id, 1, "All new.\n".into(), "r".into(), "e".into(), false)
        .unwrap();
    assert!(
        r.s.page_links(&r.admin, &r.v, &id, None, 5).unwrap().is_empty(),
        "its old links went with its words"
    );
    r.s.propose_page_revert(&r.admin, &r.v, &id, 2, 1, "r".into(), "rv".into()).unwrap();
    let back = r.s.get_wiki_page(&r.admin, &r.v, &id, None).unwrap();
    assert_eq!((back.current_revision, back.content.body.as_str()), (3, first));
    assert_eq!(back.content.blocks, v1.content.blocks);
    assert_eq!(r.s.list_page_revisions(&r.admin, &r.v, &id, None, 10).unwrap()[2].restored_from, Some(1));
    assert_eq!(r.s.page_links(&r.admin, &r.v, &id, None, 5).unwrap().len(), 2);

    propose(
        "c2",
        vec![Op::SupersedeAssertion {
            target: ObjectId("old".into()),
            base_version: 1,
            replacement: Box::new(claim("new", json!(13), "13 kg")),
        }],
    );
    let view = r.s.get_wiki_page(&r.admin, &r.v, &id, None).unwrap();
    assert_eq!(view.claims[0].status, ClaimRefStatus::Superseded);
    let current = view.claims[0].current.as_ref().unwrap();
    assert_eq!((&current.assertion.value, current.evidence[0].evidence.quote.as_str()), (&json!(13), "13 kg"));

    let broken = r.s.unresolved_page_links(&r.admin, &r.v, None, 10).unwrap();
    assert_eq!(broken.iter().map(|b| b.target.as_str()).collect::<Vec<_>>(), ["nowhere"]);

    let rows = || -> Vec<String> {
        let dump = dump_store(&r.s.engine().store().snapshot(&r.v).unwrap()).unwrap();
        dump.text
            .lines()
            .filter(|l| l.starts_with('{') && !l.starts_with("{\"change_set\""))
            .map(String::from)
            .collect()
    };
    let before = rows();
    let deletion =
        r.s.propose_page_delete(&r.admin, &r.v, &other, 1, "r".into(), "d".into())
            .unwrap()
            .changeset
            .unwrap();
    r.s.approve(&r.admin, &r.v, &deletion.header.id, None).unwrap();
    assert!(matches!(r.s.get_wiki_page(&r.admin, &r.v, &other, None), Err(ServerError::NotFound)));
    assert!(r.s.unresolved_page_links(&r.admin, &r.v, None, 10).unwrap().iter().any(|b| b.target == "other"));
    r.s.revert(&r.admin, &r.v, &deletion.header.id, "undo".into()).unwrap();
    assert_eq!(rows(), before, "every row is back");

    let target = r.s.create_vault(&r.admin, "pg-page-copy").unwrap().id;
    for (i, page) in [&id, &other].into_iter().enumerate() {
        let file = r.s.export_page(&r.admin, &r.v, page).unwrap();
        let out = r.s.import_page(&r.admin, &target, &file, None, "r".into(), format!("i{i}")).unwrap();
        let copy = r.s.get_wiki_page(&r.admin, &target, &out.page_id, None).unwrap();
        let source = r.s.get_wiki_page(&r.admin, &r.v, page, None).unwrap();
        assert_eq!(copy.content.body, source.content.body);
        assert_eq!(copy.current_slug, source.current_slug);
        let names = |v: &VaultId, p: &ObjectId| {
            let mut l: Vec<_> =
                r.s.page_links(&r.admin, v, p, None, 10)
                    .unwrap()
                    .into_iter()
                    .map(|x| (x.kind, x.target))
                    .collect();
            l.sort();
            l
        };
        assert_eq!(names(&target, &out.page_id), names(&r.v, page));
    }
    r.s.delete_vault(&r.admin, &target).unwrap();
}
