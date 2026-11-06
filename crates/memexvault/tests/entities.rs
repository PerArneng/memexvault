//! The entity registry on real Postgres, tantivy and pgvector. Run with `MEMEX_TEST_DATABASE_URL` set, as in AGENTS.md.
// These tests read the environment, measure time and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::testing::{FakeEmbedder, FakeRasterizer, FakeReranker, ManualClock, MemBlobStore, MemRegistry};
use memex_core::{CommitItem, EntityEdit, FieldInput, NewEntity, SearchEdges, Server, ServerEdges, Snapshot, Store, dump_store};
use memex_lexical::TantivyLexical;
use memex_store_postgres::PgStore;
use memex_types::{
    ChangeSetState, DocKind, EntityField, EntityRow, Event, FieldFilter, FieldLookup, FieldOp, FilterValue, Header, ItemKind, Mutation, ObjectId, Row,
    SearchMode, SearchRequest, Status, VaultId,
};
use memex_vector::PgVectors;
use memexvault::edges::{PgVectorIndex, SystemClock, SystemSeed, TantivyIndex};
use memexvault::pg_edge::PostgresStore;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    pg: Arc<PgStore>,
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
    let dir = std::env::temp_dir().join(format!("memex-entities-{name}-{}", std::process::id()));
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
    Rig { s, admin, v, pg, dir }
}

fn gadget() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["name"], "properties": {
        "name": {"type": "string", "identity": true, "searchable": true, "filterable": true},
        "category": {"type": "string", "filterable": true},
        "price": {"type": "number", "filterable": true},
        "released": {"type": "string", "format": "date", "filterable": true},
        "notes": {"type": "string", "searchable": true}
    }})
}

fn field(name: &str, value: Value) -> FieldInput {
    FieldInput {
        field: name.into(),
        value,
        evidence: vec![],
        replaces: None,
    }
}

fn new_gadget(r: &Rig, key: &str, fields: Vec<FieldInput>) -> ObjectId {
    let (id, o) =
        r.s.propose_new_entity(
            &r.admin,
            &r.v,
            NewEntity {
                type_name: "Gadget".into(),
                aliases: vec![],
                fields,
                rationale: "r".into(),
                idempotency_key: key.into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(o.changeset.unwrap().state, ChangeSetState::Committed, "{:?}", o.validators);
    id
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() as f64 * p) as usize).min(sorted.len() - 1)]
}

fn report(what: &str, mut ms: Vec<f64>) -> f64 {
    ms.sort_by(f64::total_cmp);
    let (p50, p95, p99, max) = (percentile(&ms, 0.5), percentile(&ms, 0.95), percentile(&ms, 0.99), ms[ms.len() - 1]);
    println!("{what}: n={} p50={p50:.2} ms p95={p95:.2} ms p99={p99:.2} ms max={max:.2} ms", ms.len());
    p95
}

/// Verification 1: a brand-new type on a server that was already running, with the production projector threads. Nothing is restarted or rebuilt.
#[test]
#[ignore = "needs Postgres"]
fn pg_entity_new_type_is_found_within_the_lag_target_without_a_restart() {
    let r = rig("pg-entity-lag");
    let stop = Arc::new(AtomicBool::new(false));
    let threads = [
        memexvault::workers::start_projector(r.s.clone(), stop.clone()),
        memexvault::workers::start_vector_projector(r.s.clone(), stop.clone()),
    ];
    // The indexes are live before the type exists, as on a server already in use.
    let deadline = Instant::now() + Duration::from_secs(30);
    while r.s.search_lexical(&r.admin, &r.v, "nothing", Default::default(), 1).is_err() {
        assert!(Instant::now() < deadline, "the keyword index never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    r.s.propose_entity_type(&r.admin, &r.v, "Gadget", None, gadget(), "r".into(), "t".into())
        .unwrap();
    let mut lags = vec![];
    for i in 0..40 {
        let word = format!("quixotic{i}");
        let started = Instant::now();
        let id = new_gadget(
            &r,
            &format!("e{i}"),
            vec![
                field("name", json!(format!("Gadget number {i}"))),
                field("notes", json!(word)),
                field("price", json!(i)),
            ],
        );
        // Exact and filtered lookups come from the store, so they answer at once.
        let f = ("price".to_string(), "eq".to_string(), i.to_string());
        assert_eq!(r.s.list_entities(&r.admin, &r.v, "Gadget", &[f], None, 5).unwrap()[0].header.id, id);
        loop {
            let found = r.s.search_lexical(&r.admin, &r.v, &word, Default::default(), 5).unwrap();
            if found.hits.iter().any(|h| h.id == id && h.kind == DocKind::Entity) {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(30), "entity {i} never reached the keyword index");
            std::thread::sleep(Duration::from_millis(5));
        }
        lags.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let p95 = report("commit to keyword hit (busy machine)", lags);
    assert!(p95 <= 1000.0, "lag p95 {p95} ms is over the 1 s target");
    // Hybrid search, a type filter, an exact id and a field scope through the one search API.
    let mut q = SearchRequest::new("quixotic7");
    q.filter.types = vec!["Gadget".into()];
    q.filter.kinds = vec![DocKind::Entity];
    let p = r.s.search(&r.admin, &r.v, &q).unwrap();
    let id = p.items.iter().find(|i| i.kind == ItemKind::Entity).expect("a typed hit").id.clone();
    assert!(p.degraded.is_empty(), "{:?}", p.degraded);
    let exact = r.s.search(&r.admin, &r.v, &SearchRequest::new(id.0.clone())).unwrap();
    assert_eq!(exact.path, memex_types::QueryPath::Exact);
    assert_eq!(exact.items[0].id, id);
    let mut scoped = SearchRequest::new("quixotic7");
    scoped.filter.attr = Some(memex_types::AttrScope {
        type_name: "Gadget".into(),
        field: "notes".into(),
    });
    assert_eq!(r.s.search(&r.admin, &r.v, &scoped).unwrap().items[0].id, id);
    scoped.filter.attr = Some(memex_types::AttrScope {
        type_name: "Gadget".into(),
        field: "name".into(),
    });
    assert!(
        r.s.search(&r.admin, &r.v, &scoped).unwrap().items.iter().all(|i| i.id != id),
        "the word isn't in that field"
    );
    // The rendered card reaches the vector index once its embed job runs, as with any new text.
    let started = Instant::now();
    let mut vector = SearchRequest::new("quixotic7");
    vector.mode = SearchMode::Vector;
    vector.filter.kinds = vec![DocKind::Entity];
    loop {
        while let Some(job) = r.s.claim_job(&r.v, memex_core::EMBED_JOB, "t").unwrap() {
            r.s.run_embed_job(&r.v, &job).unwrap();
            assert!(r.s.complete_job(&r.v, &job, vec![]).unwrap());
        }
        if r.s.search(&r.admin, &r.v, &vector).unwrap().items.iter().any(|i| i.kind == ItemKind::Entity) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "no entity reached the vector index");
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("embedding and vector index took {} ms for 40 entities", started.elapsed().as_millis());
    stop.store(true, Ordering::Relaxed);
    for t in threads {
        t.join().unwrap();
    }
}

/// Verification 5 on the real store: everything about the row, including the field table, comes back.
#[test]
#[ignore = "needs Postgres"]
fn pg_entity_merge_and_revert_restore_rows_values_and_redirects() {
    let r = rig("pg-entity-merge");
    r.s.propose_entity_type(&r.admin, &r.v, "Gadget", None, gadget(), "r".into(), "t".into())
        .unwrap();
    let a = new_gadget(
        &r,
        "a",
        vec![field("name", json!("Alpha")), field("category", json!("Pumps")), field("price", json!(10))],
    );
    let b = new_gadget(
        &r,
        "b",
        vec![field("name", json!("Beta")), field("category", json!("Valves")), field("price", json!(20))],
    );
    let store = r.s.engine().store();
    let dump_entities = || -> Vec<Row> {
        let snap = store.snapshot(&r.v).unwrap();
        snap.rows(memex_types::Table::Entity, None).unwrap()
    };
    let category = |c: &str| vec![("category".to_string(), "eq".to_string(), c.to_string())];
    let found = |c: &str| -> Vec<ObjectId> {
        r.s.list_entities(&r.admin, &r.v, "Gadget", &category(c), None, 10)
            .unwrap()
            .into_iter()
            .map(|e| e.header.id)
            .collect()
    };
    let before = dump_entities();
    assert_eq!(found("pumps"), std::slice::from_ref(&a));
    let o = r.s.propose_merge(&r.admin, &r.v, &a, &b, "same".into(), "m".into()).unwrap();
    let cs = o.changeset.unwrap();
    assert_eq!(cs.state, ChangeSetState::Pending);
    assert_eq!(dump_entities(), before, "nothing changes before approval");
    r.s.approve(&r.admin, &r.v, &cs.header.id, None).unwrap();
    assert_eq!(r.s.entity_redirect(&r.admin, &r.v, &a).unwrap().current_id, b);
    assert_eq!(r.s.get_entity(&r.admin, &r.v, &a).unwrap().entity.header.id, b);
    assert!(
        found("pumps").is_empty(),
        "the winner's newest value is current, and the merged row has no values"
    );
    assert_eq!(found("valves").len() + found("pumps").len(), 1);
    r.s.revert(&r.admin, &r.v, &cs.header.id, "no".into()).unwrap();
    assert_eq!(dump_entities(), before, "every row is back exactly");
    assert_eq!(found("pumps"), std::slice::from_ref(&a), "and so are the values filters read");
    assert_eq!(found("valves"), std::slice::from_ref(&b));
    assert_eq!(r.s.entity_redirect(&r.admin, &r.v, &a).unwrap().current_id, a);
    // The log replays to the same state on the fake, which is the portable-export promise.
    let snap = store.snapshot(&r.v).unwrap();
    let copy = memex_core::testing::MemStore::new();
    let lease = copy.open_vault(&r.v).unwrap();
    memex_core::replay(memex_core::all_events(&snap).unwrap(), &copy, &lease).unwrap();
    assert_eq!(dump_store(&snap).unwrap(), dump_store(&copy.snapshot(&r.v).unwrap()).unwrap());
    drop(snap);
    // And an edit commits through a real transaction, with its evidence-free claim, its row and values together.
    let edit = EntityEdit {
        base_version: 1,
        aliases: Some(vec!["Alfa".into()]),
        fields: vec![field("released", json!("2026-02-03"))],
        rationale: "r".into(),
        idempotency_key: "e".into(),
    };
    r.s.propose_entity_edit(&r.admin, &r.v, &a, edit, false).unwrap();
    let f = vec![("released".to_string(), "gte".to_string(), "2026-01-01".to_string())];
    assert_eq!(r.s.list_entities(&r.admin, &r.v, "Gadget", &f, None, 10).unwrap()[0].header.id, a);
    let resolved = r.s.resolve_mention(&r.admin, &r.v, "ALFA", Some("Gadget"), 5).unwrap();
    assert_eq!(resolved.candidates[0].matched_on.as_deref(), Some("$alias"));
    let _ = &r.pg;
}

fn text(t: String) -> FieldLookup {
    FieldLookup {
        text: Some(t),
        number: None,
        time_ms: None,
    }
}

const FIELDS_PER_ENTITY: usize = 5;
const CATEGORIES: usize = 1000;

fn bench_entity(i: usize) -> EntityRow {
    let header = Header {
        id: ObjectId(format!("{i:032x}")),
        creator: "bench".into(),
        status: Status::Active,
        version: 1,
        acl_id: None,
    };
    let f = |name: &str, value: Value, lookups: Vec<FieldLookup>| EntityField {
        field: name.into(),
        value,
        assertion_id: ObjectId(format!("{name}{i:028x}")),
        why: "it's the only claim on this field".into(),
        others: vec![],
        lookups,
    };
    let category = format!("category {}", i % CATEGORIES);
    let price = (i * 7919 % 100_000) as i64;
    let at = (i * 86_400_000 % (1 << 40)) as i64;
    EntityRow {
        header,
        type_name: "Gadget".into(),
        type_version: 1,
        aliases: vec![format!("Alias {i}")],
        alias_keys: vec![format!("alias {i}")],
        fields: vec![
            f("name", json!(format!("Gadget {i}")), vec![text(format!("gadget {i}"))]),
            f("category", json!(category), vec![text(category.clone())]),
            f(
                "price",
                json!(price),
                vec![FieldLookup {
                    text: None,
                    number: Some(price.into()),
                    time_ms: None,
                }],
            ),
            f(
                "released",
                json!("2026-01-01"),
                vec![FieldLookup {
                    text: None,
                    number: None,
                    time_ms: Some(at),
                }],
            ),
        ],
        merged_from: vec![],
        redirect_to: None,
        changeset_id: ObjectId("bench".into()),
        updated_at_ms: 0,
    }
}

/// Spec 023's item 6. Rows go straight to the store, as loading 1M values through proposals would take hours and measure nothing about filters.
#[test]
#[ignore = "needs Postgres; a few minutes and about 2 GB in its own database"]
fn entity_filter_bench() {
    let r = rig("entity-bench");
    r.s.propose_entity_type(&r.admin, &r.v, "Gadget", None, gadget(), "r".into(), "t".into())
        .unwrap();
    let store = r.s.engine().store();
    let lease = r.s.vault_lease_for_tests(&r.v).unwrap();
    let mut head = store.snapshot(&r.v).unwrap().head().unwrap().unwrap();
    let mut loaded = 0usize;
    let mut results = vec![];
    // Loading 1M takes minutes, so `MEMEX_ENTITY_BENCH_MAX` can stop earlier.
    let max: usize = std::env::var("MEMEX_ENTITY_BENCH_MAX").map_or(1_000_000, |m| m.parse().expect("a number"));
    for values in [50_000usize, 200_000, 1_000_000].into_iter().filter(|v| *v <= max) {
        let entities = values / FIELDS_PER_ENTITY;
        let started = Instant::now();
        while loaded < entities {
            let upto = (loaded + 2000).min(entities);
            let mutations: Vec<Mutation> = (loaded..upto).map(|i| Mutation::Put(Row::Entity(bench_entity(i)))).collect();
            head += 1;
            let event = Event {
                seq: head,
                vault: r.v.clone(),
                actor: "bench".into(),
                time_ms: 0,
                kind: "state.recorded".into(),
                schema_version: 1,
                payload: json!({"mutations": []}),
            };
            store
                .commit_batch(
                    &lease,
                    &[CommitItem {
                        event: &event,
                        mutations: &mutations,
                    }],
                )
                .unwrap();
            loaded = upto;
        }
        println!("{values} values: loaded to {entities} entities in {:.1} s", started.elapsed().as_secs_f64());
        let schema = memex_store_postgres::schema_of_vault(&r.v);
        let mut conn = r.pg.take_conn().unwrap();
        conn.client
            .batch_execute(&format!("ANALYZE {schema}.entity_fields; ANALYZE {schema}.entities;"))
            .unwrap();
        let counted: i64 = conn
            .client
            .query_one(&format!("SELECT count(*) FROM {schema}.entity_fields"), &[])
            .unwrap()
            .get(0);
        let size: i64 = conn
            .client
            .query_one(&format!("SELECT pg_total_relation_size('{schema}.entity_fields')"), &[])
            .unwrap()
            .get(0);
        r.pg.give_conn(conn);
        println!("field table: {counted} rows, {} MB with its indexes", size / (1 << 20));
        assert_eq!(counted as usize, values);
        // Controls, to tell a filter's cost from what every read pays: a snapshot opened and closed, and one row by primary key inside it.
        let mut bare = Vec::with_capacity(2000);
        let mut by_key = Vec::with_capacity(2000);
        for i in 0..2000 {
            let t = Instant::now();
            drop(store.snapshot(&r.v).unwrap());
            bare.push(t.elapsed().as_secs_f64() * 1000.0);
            let key = memex_types::RowKey {
                table: memex_types::Table::Entity,
                id: ObjectId(format!("{:032x}", i * 7907 % entities)),
            };
            let t = Instant::now();
            let snap = store.snapshot(&r.v).unwrap();
            assert!(snap.get(&key).unwrap().is_some());
            drop(snap);
            by_key.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        let p95 = report(&format!("{values} values, control, snapshot only"), bare);
        results.push((values, "control snapshot only".to_string(), p95));
        let p95 = report(&format!("{values} values, control, one row by id"), by_key);
        results.push((values, "control one row by id".to_string(), p95));
        let results_of_lanes = &mut results;
        let mut lane = |what: &str, make: &dyn Fn(usize) -> Vec<FieldFilter>| {
            let mut ms = Vec::with_capacity(2000);
            let mut matched = 0;
            for i in 0..2000 {
                let filters = make(i * 7907 % entities);
                let t = Instant::now();
                let snap = store.snapshot(&r.v).unwrap();
                let rows = snap.entities_matching("Gadget", &filters, None, 20).unwrap();
                drop(snap);
                ms.push(t.elapsed().as_secs_f64() * 1000.0);
                matched += rows.len();
            }
            let p95 = report(&format!("{values} values, store, {what} (avg {} rows)", matched / 2000), ms);
            results_of_lanes.push((values, what.to_string(), p95));
        };
        let eq = |field: &str, t: String| FieldFilter {
            field: field.into(),
            op: FieldOp::Eq,
            value: FilterValue::Text(t),
        };
        lane("exact unique", &|i| vec![eq("name", format!("gadget {i}"))]);
        lane("exact, ~0.1% of entities", &|i| vec![eq("category", format!("category {}", i % CATEGORIES))]);
        lane("range, ~0.1%", &|i| {
            vec![FieldFilter {
                field: "price".into(),
                op: FieldOp::Lt,
                value: FilterValue::Number((i % 100) as f64),
            }]
        });
        lane("range, ~50%", &|_| {
            vec![FieldFilter {
                field: "price".into(),
                op: FieldOp::Lt,
                value: FilterValue::Number(50_000.0),
            }]
        });
        lane("exact and range", &|i| {
            vec![
                eq("category", format!("category {}", i % CATEGORIES)),
                FieldFilter {
                    field: "price".into(),
                    op: FieldOp::Lt,
                    value: FilterValue::Number(50_000.0),
                },
            ]
        });
        // The query alone, inside a snapshot already open, with what every read pays taken out.
        let mut alone = Vec::with_capacity(2000);
        {
            let snap = store.snapshot(&r.v).unwrap();
            for i in 0..2000 {
                let filters = [eq("category", format!("category {}", i % CATEGORIES))];
                let t = Instant::now();
                let rows = snap.entities_matching("Gadget", &filters, None, 20).unwrap();
                alone.push(t.elapsed().as_secs_f64() * 1000.0);
                assert!(!rows.is_empty());
            }
        }
        let p95 = report(&format!("{values} values, query alone, exact, ~0.1%"), alone);
        results.push((values, "query alone exact".to_string(), p95));
        // The same exact filter as the API runs it: access checks, the type's fields, parsing, then the query.
        let mut ms = Vec::with_capacity(2000);
        for i in 0..2000 {
            let f = vec![("category".to_string(), "eq".to_string(), format!("category {}", i % CATEGORIES))];
            let t = Instant::now();
            let rows = r.s.list_entities(&r.admin, &r.v, "Gadget", &f, None, 21).unwrap();
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
            assert!(!rows.is_empty());
        }
        let p95 = report(&format!("{values} values, facade, exact, ~0.1%"), ms);
        results.push((values, "facade exact".to_string(), p95));
    }
    println!("--- summary, p95 in ms, on a machine shared with other workers ---");
    for (values, what, p95) in &results {
        println!("{values:>8} {what}: {p95:.2}");
    }
    let at_200k = results
        .iter()
        .find(|(v, w, _)| *v == 200_000 && w == "exact, ~0.1% of entities")
        .map(|x| x.2)
        .unwrap();
    println!("the target is exact filter p95 <= 2 ms at 200k values: measured {at_200k:.2} ms");
    let _ = lease;
}
