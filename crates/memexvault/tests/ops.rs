//! Spec 030 on real Postgres, blob folders and tantivy: erasure down to the bytes on disk, backup during ingest restored elsewhere, and an index rebuilt after its folder is deleted. Scripted agents only, so no LLM is called.
//!
//! Run with `MEMEX_TEST_DATABASE_URL` naming a database of your own on a server where that user is a superuser, as the erasure test reads Postgres's data files with `pg_read_binary_file` and runs `CHECKPOINT`. The backup test makes a second database beside it, named with `_restore`.
// These tests read the environment and print what they measure.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_blob_fs::{FsBlobs, RealFs};
use memex_core::testing::{ChatCall, FakeEmbedder, FakeRasterizer, FakeReranker, ManualClock, MemRegistry, ScriptedAgent, ScriptedChat};
use memex_core::{
    ChatInput, ChatLimits, ChatSettingsUpdate, EXTRACT_JOB, NewPage, Proposal, RENDITION_JOB, RenditionInput, RestoreTarget, SearchEdges, Server, ServerEdges,
    Snapshot, StepOutcome, Store, UploadLimits, UploadRequest, all_events, dump_store, replay,
};
use memex_lexical::TantivyLexical;
use memex_store_postgres::{PgStore, schema_of_vault};
use memex_types::{
    AgentError, AgentReply, AgentRequest, AgentUsage, ChangeSetState, ERASE_JOB, Epistemic, EvidenceInput, ObjectId, Op, RiskDecision, Row, SearchMode,
    SearchRequest, Stance, Table, VaultId,
};
use memex_vector::PgVectors;
use memexvault::edges::{FsBlobStore, PgVectorIndex, SystemSeed, TantivyIndex};
use memexvault::pg_edge::PostgresStore;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

struct Real;

impl ServerEdges for Real {
    type Caches = memex_core::NoCaches;
    type Registry = MemRegistry;
    type Store = PostgresStore;
    type Blobs = FsBlobStore<RealFs>;
    type Clock = ManualClock;
    type Seed = SystemSeed;
    type Rasterizer = FakeRasterizer;
    type Lexical = TantivyIndex;
    type Vector = PgVectorIndex;
    type Embedder = FakeEmbedder;
    type Reranker = FakeReranker;
}

const MARKER: &str = "ZQXERASEMARKER4417";

fn base_url() -> String {
    std::env::var("MEMEX_TEST_DATABASE_URL").expect("not run: MEMEX_TEST_DATABASE_URL isn't set")
}

/// The same server, another database on it, made if it isn't there.
fn database(suffix: &str) -> String {
    let url = base_url();
    let (prefix, db) = url.rsplit_once('/').expect("a database in the URL");
    let name = format!("{}{suffix}", db.split('?').next().unwrap());
    let mut c = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    if c.query_opt("SELECT 1 FROM pg_database WHERE datname = $1", &[&name]).unwrap().is_none() {
        c.batch_execute(&format!("CREATE DATABASE {name}")).unwrap();
    }
    format!("{prefix}/{name}")
}

struct Rig {
    s: Arc<Server<Real>>,
    pg: Arc<PgStore>,
    admin: memex_core::Caller,
    agent: memex_core::Caller,
    v: VaultId,
    dir: PathBuf,
    url: String,
}

impl Drop for Rig {
    fn drop(&mut self) {
        for v in self.s.vault_ids().unwrap_or_default() {
            let _ = self.s.delete_vault(&self.admin, &v);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn server(url: &str, dir: &Path) -> (Arc<Server<Real>>, Arc<PgStore>) {
    let pg = Arc::new(PgStore::connect(url, 8).expect("connect"));
    pg.migrate().expect("migrate");
    let s = Arc::new(Server::<Real>::new(
        MemRegistry::new(),
        PostgresStore(pg.clone()),
        FsBlobStore(FsBlobs::new(dir, RealFs)),
        ManualClock::starting_at(1_700_000_000_000),
        SystemSeed,
        SearchEdges {
            lexical: TantivyIndex(TantivyLexical::new(dir)),
            vector: PgVectorIndex(PgVectors::new(pg.clone())),
            embedder: FakeEmbedder::default(),
            reranker: FakeReranker::default(),
        },
    ));
    (s, pg)
}

fn rig(name: &str, url: String) -> Rig {
    let dir = std::env::temp_dir().join(format!("memex-ops-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (s, pg) = server(&url, &dir);
    s.set_extraction(Some(memex_core::ExtractionLimits::default()));
    s.set_chat(Some(ChatLimits::default()));
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, name).unwrap().id;
    let p = s.create_principal(&admin, "bot", memex_types::PrincipalKind::Agent, false).unwrap();
    let t = s
        .create_token(
            &admin,
            memex_core::NewToken {
                principal_id: Some(p.id),
                name: "t".into(),
                grants: vec![memex_types::Grant {
                    vault: v.clone(),
                    scope: memex_types::Scope::Admin,
                }],
                expires_at_ms: None,
            },
        )
        .unwrap();
    let agent = s.authenticate(&t.token).unwrap();
    Rig {
        s,
        pg,
        admin,
        agent,
        v,
        dir,
        url,
    }
}

fn filler(n: usize) -> String {
    "Detta är en mening som fyller ut stycket med vanliga ord. ".repeat(n)
}

fn first_sentence(text: &str) -> String {
    text.split_inclusive(". ").next().unwrap().trim().to_string()
}

fn reply(doc: Value) -> Result<AgentReply, AgentError> {
    Ok(AgentReply {
        json: doc.to_string(),
        usage: AgentUsage {
            input_tokens: 10,
            output_tokens: 10,
            thinking_tokens: 0,
        },
        tool_calls: 0,
    })
}

/// A claim per segment quoting its first sentence, and a Person named in the first.
fn claims_and_a_person(req: &AgentRequest) -> Result<AgentReply, AgentError> {
    let doc: Value = serde_json::from_str(req.input.split("\n\nYour previous").next().unwrap()).unwrap();
    let segments: Vec<(String, String)> = doc["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["id"].as_str().unwrap().to_string(), s["text"].as_str().unwrap().to_string()))
        .collect();
    let claims: Vec<Value> = segments
        .iter()
        .map(|(id, text)| {
            let q = first_sentence(text);
            json!({"segment_id": id, "quote": q, "subject": q, "predicate": "säger", "value": q})
        })
        .collect();
    let (id, text) = &segments[0];
    let name = first_sentence(text).trim_end_matches('.').to_string();
    reply(json!({"claims": claims, "entities": {"Person": [{"segment_id": id, "quote": first_sentence(text), "fields": {"name": name}}]}}))
}

impl Rig {
    fn sql(&self) -> postgres::Client {
        postgres::Client::connect(&self.url, postgres::NoTls).expect("connect")
    }

    /// Uploads, parses and extracts a document, and approves what extraction proposed.
    fn ingest(&self, name: &str, text: &str, extract: bool) -> ObjectId {
        let req = UploadRequest {
            name: name.into(),
            trust_tier: "trusted".into(),
            metadata: json!({}),
            replaces: None,
        };
        let source = self
            .s
            .upload(&self.admin, &self.v, req, UploadLimits::default(), &mut text.as_bytes())
            .unwrap()
            .source
            .header
            .id;
        let job = self.s.claim_job(&self.v, RENDITION_JOB, "parser").unwrap().unwrap();
        let rows = self
            .s
            .build_rendition(
                &self.v,
                &source,
                RenditionInput::Plain {
                    text: text.into(),
                    parser: "plain-1".into(),
                },
            )
            .unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
        if extract {
            let agent = ScriptedAgent::answering(claims_and_a_person);
            let job = self.s.claim_job(&self.v, EXTRACT_JOB, "w").unwrap().unwrap();
            let rows = self.s.run_extraction_job(&self.v, &job, &agent).unwrap();
            assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
            for c in self.s.list_changesets(&self.admin, &self.v, Some(ChangeSetState::Pending), None, 1000).unwrap() {
                self.s.approve(&self.admin, &self.v, &c.header.id, None).unwrap();
            }
        }
        source
    }

    fn table(&self, table: Table) -> Vec<Row> {
        let snap = self.s.engine().store().snapshot(&self.v).unwrap();
        let (mut out, mut after) = (vec![], None);
        loop {
            let page = snap.rows(table, after.as_ref()).unwrap();
            let Some(last) = page.last() else { return out };
            after = Some(last.header().id.clone());
            out.extend(page);
        }
    }

    fn segments_of(&self, source: &ObjectId) -> Vec<(ObjectId, String)> {
        self.table(Table::Segment)
            .into_iter()
            .filter_map(|r| match r {
                Row::Segment(s) if &s.source_id == source => Some((s.header.id, s.text)),
                _ => None,
            })
            .collect()
    }

    /// Runs both projectors and every embedding job until nothing is left.
    fn settle(&self) {
        for _ in 0..200 {
            let a = self.s.project_lexical(&self.v).unwrap();
            let b = self.s.project_vector(&self.v).unwrap();
            let mut embedded = false;
            while let Some(job) = self.s.claim_job(&self.v, memex_core::EMBED_JOB, "t").unwrap() {
                self.s.run_embed_job(&self.v, &job).unwrap();
                assert!(self.s.complete_job(&self.v, &job, vec![]).unwrap());
                embedded = true;
            }
            self.s.engine().clock().advance(Duration::from_secs(60));
            if a == StepOutcome::Idle && b == StepOutcome::Idle && !embedded {
                return;
            }
        }
        panic!("the projectors never settled");
    }

    fn keyword_hits(&self, text: &str) -> Vec<ObjectId> {
        let mut q = SearchRequest::new(text);
        q.mode = SearchMode::Lexical;
        self.s.search(&self.admin, &self.v, &q).unwrap().items.into_iter().map(|i| i.id).collect()
    }

    /// Stores every text and JSON column of the vault uncompressed, so a marker in a long row shows in its TOAST file. The product's default compresses, which could only hide a marker from the scan.
    fn uncompressed(&self) {
        let schema = schema_of_vault(&self.v);
        let mut c = self.sql();
        let cols = c
            .query(
                "SELECT table_name::text, column_name::text FROM information_schema.columns WHERE table_schema = $1 AND data_type IN ('text', 'jsonb')",
                &[&schema],
            )
            .unwrap();
        for r in cols {
            let (t, col): (String, String) = (r.get(0), r.get(1));
            c.batch_execute(&format!("ALTER TABLE {schema}.\"{t}\" ALTER COLUMN \"{col}\" SET STORAGE EXTERNAL"))
                .unwrap();
        }
    }
}

/// Every file under a folder, as (path, bytes).
fn files_under(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(bytes) = std::fs::read(&p) {
                out.push((p.display().to_string(), bytes));
            }
        }
    }
    out
}

/// Files holding the marker in any letter case, as tokenizers lowercase it.
fn holding(files: &[(String, Vec<u8>)], marker: &str) -> Vec<String> {
    let (upper, lower) = (marker.as_bytes().to_vec(), marker.to_lowercase().into_bytes());
    files
        .iter()
        .filter(|(_, b)| b.windows(upper.len()).any(|w| w == upper.as_slice() || w == lower.as_slice()))
        .map(|(p, _)| p.clone())
        .collect()
}

/// Every file of the Postgres server's data folder under `dirs`, read through SQL after a checkpoint, so what was in memory is on disk.
fn pg_files(c: &mut postgres::Client, dirs: &[&str]) -> Vec<(String, Vec<u8>)> {
    c.batch_execute("CHECKPOINT").unwrap();
    let mut out = vec![];
    let mut stack: Vec<String> = dirs.iter().map(|d| d.to_string()).collect();
    while let Some(d) = stack.pop() {
        let names: Vec<String> = c.query("SELECT pg_ls_dir($1, true, false)", &[&d]).unwrap().iter().map(|r| r.get(0)).collect();
        for n in names {
            let path = format!("{d}/{n}");
            let Ok(row) = c.query_one("SELECT isdir FROM pg_stat_file($1, true)", &[&path]) else {
                continue;
            };
            match row.get::<_, Option<bool>>(0) {
                Some(true) => stack.push(path),
                Some(false) => {
                    let bytes: Option<Vec<u8>> = c.query_one("SELECT pg_read_binary_file($1, true)", &[&path]).unwrap().get(0);
                    out.push((path, bytes.unwrap_or_default()));
                }
                None => {}
            }
        }
    }
    out
}

/// Items 3, 4, 5 and 6: an erasure leaves no marker in any row, event, index file, blob, chat table or Postgres data file once compacted, the rewritten log replays to the same state, an agent's proposal waits for a person, the compaction's locks are timed, and the preview lists exactly what went. The positive controls show the marker in those files first, so finding none afterwards means something. WAL is scanned and reported as the documented limit.
#[test]
#[ignore = "needs Postgres as a superuser"]
fn pg_erasure_leaves_no_marker_in_the_vault_folder_or_postgres_files() {
    let r = rig("erase", database("_erase"));
    r.uncompressed();
    let schema = schema_of_vault(&r.v);
    let s = &r.s;
    s.set_chat_settings(
        &r.admin,
        &r.v,
        ChatSettingsUpdate {
            enabled: true,
            thread_token_budget: None,
            day_token_budget: None,
        },
    )
    .unwrap();
    let marked_text = format!(
        "Anna {MARKER} byggde fartyget i Stockholm. {f}\n\nFartyget seglade med {MARKER} till Riga. {f}",
        f = filler(8)
    );
    let other_text = format!("Erik Lind ritade hamnen i Göteborg. {f}\n\nHamnen fick en ny kaj år 1890. {f}", f = filler(8));
    let marked = r.ingest("marked.txt", &marked_text, true);
    let other = r.ingest("other.txt", &other_text, true);
    // A person's claim on both documents, a page citing a claim from the marked one, and a chat that read it.
    let (m_seg, m_text) = r.segments_of(&marked)[0].clone();
    let (o_seg, o_text) = r.segments_of(&other)[0].clone();
    let evidence = |seg: ObjectId, text: &str| EvidenceInput {
        segment_id: seg,
        quote: first_sentence(text),
        stance: Stance::Supports,
    };
    let out = s
        .propose(
            &r.admin,
            &r.v,
            Proposal {
                ops: vec![Op::AddAssertion {
                    id: ObjectId("mixed-claim".into()),
                    subject: "hamnar".into(),
                    predicate: "jämförs".into(),
                    value: json!("två"),
                    epistemic: Epistemic::Asserted,
                    machine: false,
                    confidence_permille: 900,
                    valid_from_ms: None,
                    valid_to_ms: None,
                    evidence: vec![
                        (ObjectId("ev-m".into()), evidence(m_seg.clone(), &m_text)),
                        (ObjectId("ev-o".into()), evidence(o_seg, &o_text)),
                    ],
                }],
                rationale: "both".into(),
                idempotency_key: "mixed".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(out.risk, RiskDecision::AutoCommit, "{:?}", out.validators);
    let cited = r
        .table(Table::Assertion)
        .into_iter()
        .find_map(|row| match row {
            Row::Assertion(a) if a.subject.contains(MARKER) => Some(a.header.id),
            _ => None,
        })
        .unwrap();
    let (page, _) = s
        .propose_new_page(
            &r.admin,
            &r.v,
            NewPage {
                title: "Notes".into(),
                slug: None,
                body: format!("A claim: {{{{claim:{cited}}}}} and nothing more."),
                rationale: "r".into(),
                idempotency_key: "page".into(),
            },
            false,
        )
        .unwrap();
    let chat = ScriptedChat::new(vec![vec![
        ChatCall::calls("read", json!({"kind": "segment", "id": m_seg.0})),
        ChatCall::says("It names someone."),
    ]]);
    let prepared = s
        .prepare_chat(
            &r.admin,
            &r.v,
            ChatInput {
                thread_id: "t1".into(),
                run_id: "r1".into(),
                message: "What does it say?".into(),
            },
            memex_core::new_chat_cancel(),
        )
        .unwrap();
    s.run_chat(prepared, &chat, memex_api::chat_tool::<Real>, |_| true);
    assert!(chat.tool_answers()[0].to_string().contains(MARKER));
    r.settle();
    assert!(!r.keyword_hits(MARKER).is_empty());

    // The controls: the marker is in the vault's folder and in Postgres's files, found by reading them.
    let mut c = r.sql();
    let db_oid: u32 = c
        .query_one("SELECT oid FROM pg_database WHERE datname = current_database()", &[])
        .unwrap()
        .get(0);
    let base = format!("base/{db_oid}");
    // Tantivy compresses its term dictionaries and doesn't store the text, so its files are read through tantivy, segment by segment, where a deleted document's terms stay until a merge.
    let index_scan = || TantivyLexical::new(&r.dir).scan_files(&r.v, MARKER).unwrap();
    let folder_before = holding(&files_under(&r.dir), MARKER);
    let (terms_before, _) = index_scan();
    let pg_before = holding(&pg_files(&mut c, &[&base, "global"]), MARKER);
    println!(
        "before: marker in {} vault files, {} index terms, {} Postgres files",
        folder_before.len(),
        terms_before.len(),
        pg_before.len()
    );
    assert!(folder_before.iter().any(|p| p.contains("/blobs/")), "{folder_before:?}");
    assert!(!terms_before.is_empty());
    assert!(!pg_before.is_empty());

    // Item 6's preview, and item 5: an agent's proposal waits, and an agent can't approve it.
    let preview = s.erasure_preview(&r.admin, &r.v, &marked).unwrap();
    let proposed = s.propose_erasure(&r.agent, &r.v, &marked, "forget it".into()).unwrap();
    let cs = proposed.changeset.unwrap();
    assert_eq!((proposed.risk, cs.state), (RiskDecision::Review, ChangeSetState::Pending));
    assert!(matches!(
        s.approve(&r.agent, &r.v, &cs.header.id, None),
        Err(memex_core::ServerError::Forbidden)
    ));
    let started = Instant::now();
    s.approve(&r.admin, &r.v, &cs.header.id, None).unwrap();
    let commit_ms = started.elapsed().as_millis();
    r.settle();
    let job = s.claim_job(&r.v, ERASE_JOB, "w").unwrap().unwrap();
    let started = Instant::now();
    let rows = s.run_erase_job(&r.v, &job).unwrap();
    assert!(s.complete_job(&r.v, &job, rows).unwrap());
    let cascade_ms = started.elapsed().as_millis();
    let record = s.erasures(&r.admin, &r.v, None, 10).unwrap().remove(0).record;
    assert_eq!(
        (&record.removed, &record.rewritten, &record.flagged_claims, &record.flagged_pages),
        (&preview.removed, &preview.rewritten, &preview.flagged_claims, &preview.flagged_pages),
        "the preview listed exactly what went"
    );
    assert_eq!(record.flagged_pages, vec![page]);
    assert_eq!(record.flagged_claims, vec![ObjectId("mixed-claim".into())]);
    let snap = s.engine().store().snapshot(&r.v).unwrap();
    for k in &record.removed {
        assert!(snap.get(k).unwrap().is_none(), "{k:?}");
    }
    drop(snap);

    // Item 5: how long VACUUM FULL holds each table's exclusive lock, timed table by table on the erased vault.
    let tables: Vec<String> = c
        .query("SELECT tablename::text FROM pg_tables WHERE schemaname = $1 ORDER BY tablename", &[&schema])
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    let mut locks = vec![];
    for t in &tables {
        let at = Instant::now();
        c.batch_execute(&format!("VACUUM FULL {schema}.\"{t}\"")).unwrap();
        locks.push((t.clone(), at.elapsed().as_micros()));
    }
    locks.sort_by_key(|(_, us)| std::cmp::Reverse(*us));
    let total: u128 = locks.iter().map(|(_, us)| us).sum();
    println!(
        "approve and commit {commit_ms} ms, cascade with compaction {cascade_ms} ms; VACUUM FULL of {} tables {} ms in all, longest {:?}",
        tables.len(),
        total / 1000,
        &locks[..3.min(locks.len())]
    );

    // Item 3: no API or search hit, and no byte anywhere outside the WAL.
    assert!(r.keyword_hits(MARKER).is_empty());
    let any = s.search(&r.admin, &r.v, &SearchRequest::new(MARKER)).unwrap().items;
    assert!(any.iter().all(|i| !i.text.contains(MARKER)));
    assert!(matches!(s.get_source(&r.admin, &r.v, &marked), Err(memex_core::ServerError::NotFound)));
    assert!(matches!(s.lineage(&r.admin, &r.v, &cited), Err(memex_core::ServerError::NotFound)));
    assert!(
        s.list_chat_threads(&r.admin, &r.v, None, 10).unwrap().is_empty(),
        "the thread that read it is gone"
    );
    let folder_after = holding(&files_under(&r.dir), MARKER);
    let (terms_after, stray) = index_scan();
    assert!(terms_after.is_empty(), "{terms_after:?}");
    assert!(stray.is_empty(), "files no segment owns: {stray:?}");
    let pg_after = holding(&pg_files(&mut c, &[&base, "global"]), MARKER);
    let wal = holding(&pg_files(&mut c, &["pg_wal"]), MARKER);
    println!(
        "after: marker in {} vault files, {} Postgres data files, {} WAL files (the documented limit)",
        folder_after.len(),
        pg_after.len(),
        wal.len()
    );
    assert!(folder_after.is_empty(), "{folder_after:?}");
    assert!(pg_after.is_empty(), "{pg_after:?}");

    // Item 4: the rewritten log replays into an empty store to the same state.
    let snap = s.engine().store().snapshot(&r.v).unwrap();
    let events = all_events(&snap).unwrap();
    let live = dump_store(&snap).unwrap();
    drop(snap);
    assert!(!events.iter().any(|e| e.payload.to_string().contains(MARKER)));
    let mem = memex_core::testing::MemStore::new();
    let lease = mem.open_vault(&r.v).unwrap();
    replay(events, &mem, &lease).unwrap();
    assert_eq!(dump_store(&mem.snapshot(&r.v).unwrap()).unwrap().sha256, live.sha256);
    let report = s.reconcile(&r.admin, &r.v, None, true, false).unwrap();
    assert!(report.clean(), "{report:?}");
}

/// Item 1: a backup taken while documents are being ingested restores into another database, and reconcile finds no difference there or between it and the original at the backup's event.
#[test]
#[ignore = "needs Postgres"]
fn pg_backup_during_ingest_restores_elsewhere_with_no_differences() {
    let a = rig("backup", database("_backup"));
    for i in 0..5 {
        a.ingest(&format!("doc-{i}.txt"), &format!("Dokument {i} handlar om hamnen. {}", filler(4)), true);
    }
    a.settle();
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (s, admin, v, stop) = (a.s.clone(), a.admin.clone(), a.v.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut n = 0;
            while !stop.load(Ordering::Relaxed) {
                let req = UploadRequest {
                    name: format!("during-{n}.txt"),
                    trust_tier: "trusted".into(),
                    metadata: json!({}),
                    replaces: None,
                };
                s.upload(
                    &admin,
                    &v,
                    req,
                    UploadLimits::default(),
                    &mut format!("Under säkerhetskopieringen, nummer {n}.").as_bytes(),
                )
                .unwrap();
                n += 1;
            }
            n
        })
    };
    std::thread::sleep(Duration::from_millis(300));
    let path = a.dir.join("vault.backup");
    let started = Instant::now();
    let (header, summary) = {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        a.s.backup_vault(&a.v, &mut out).unwrap()
    };
    let backup_ms = started.elapsed().as_millis();
    std::thread::sleep(Duration::from_millis(300));
    stop.store(true, Ordering::Relaxed);
    let uploads = writer.join().unwrap();
    let head_after = a.s.engine().store().snapshot(&a.v).unwrap().head().unwrap();
    println!(
        "backup during {uploads} uploads: event {:?} of {head_after:?} at the end, {} events, {} blobs, {} embeddings, {} bytes, {backup_ms} ms",
        header.head, summary.events, summary.blobs, summary.embeddings, summary.bytes
    );
    assert!(header.head < head_after, "ingest went on past the backup");

    // Elsewhere: another database, folder and server.
    let b_url = database("_restore");
    let b_dir = a.dir.join("elsewhere");
    let (b, _pg) = server(&b_url, &b_dir);
    let b_admin = b.authenticate(&b.bootstrap("root").unwrap().token).unwrap();
    for v in b.vault_ids().unwrap() {
        let _ = b.delete_vault(&b_admin, &v);
    }
    let file = path.clone();
    let started = Instant::now();
    let (row, restored) = b
        .restore_vault(
            "operator",
            &mut || Ok(Box::new(std::fs::File::open(&file)?) as Box<dyn std::io::Read>),
            RestoreTarget::Original,
            None,
        )
        .unwrap();
    let restore_ms = started.elapsed().as_millis();
    assert_eq!(row.id, a.v);
    assert_eq!(restored.sha256, header.dump_sha256);
    let elsewhere = b.reconcile(&b_admin, &row.id, None, true, false).unwrap();
    assert!(elsewhere.clean(), "{elsewhere:?}");
    assert_eq!(elsewhere.log.head, header.head);
    let original = a.s.reconcile(&a.admin, &a.v, header.head, false, false).unwrap();
    assert!(original.clean(), "{original:?}");
    assert_eq!(original.log.at.as_ref().map(|(_, h)| h.as_str()), Some(elsewhere.log.tables_sha256.as_str()));
    println!(
        "restored in {restore_ms} ms; tables {} on both at event {:?}",
        elsewhere.log.tables_sha256, header.head
    );
    // A restore never overwrites what's there unless told to.
    assert!(matches!(
        b.restore_vault(
            "operator",
            &mut || Ok(Box::new(std::fs::File::open(&path)?) as Box<dyn std::io::Read>),
            RestoreTarget::Original,
            None,
        ),
        Err(memex_core::ServerError::Conflict(_))
    ));
    let _ = b.delete_vault(&b_admin, &row.id);
}

/// Item 2: deleting the keyword index's folder and restarting rebuilds it, with the same results for the same queries.
#[test]
#[ignore = "needs Postgres"]
fn pg_deleting_the_index_folder_and_restarting_rebuilds_it_the_same() {
    let r = rig("reindex", database("_reindex"));
    let n = std::env::var("MEMEX_OPS_DOCS").ok().and_then(|v| v.parse().ok()).unwrap_or(40usize);
    for i in 0..n {
        let text = format!(
            "Dokument {i} om hamnen i {}. {}\n\nKajen nummer {i} byggdes av {}. {}",
            ["Göteborg", "Stockholm", "Malmö", "Riga"][i % 4],
            filler(3),
            ["Anna", "Erik", "Olof"][i % 3],
            filler(3)
        );
        r.ingest(&format!("doc-{i}.txt"), &text, false);
    }
    r.settle();
    // Each matches at most 1,000 documents, the most a search returns, so no page cuts through a tie.
    let queries = ["Göteborg", "Anna", "Riga", "Göteborg Anna", "\"nummer 7\"", "Malmö Olof"];
    // Every hit with its score, as ties come in id order there, where a page of 20 would cut through a tie wherever the index's own order puts it.
    let hits = |s: &Server<Real>, c: &memex_core::Caller| -> Vec<Vec<(ObjectId, i64)>> {
        queries
            .iter()
            .map(|q| {
                s.search_lexical(c, &r.v, q, memex_types::LexicalFilter::default(), 1000)
                    .unwrap()
                    .hits
                    .into_iter()
                    .map(|h| (h.id, (h.score * 10_000.0).round() as i64))
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    // Scores agree to 4 decimals. Below that, summing BM25 over one segment or several differs in the last bits, which can swap two hits whose scores differ by less, so those are put in id order.
    let ordered = |mut all: Vec<Vec<(ObjectId, i64)>>| -> Vec<Vec<(ObjectId, i64)>> {
        for list in &mut all {
            list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        }
        all
    };
    let as_followed = hits(&r.s, &r.admin);
    // A followed index still counts the old copies of updated documents in its BM25 statistics until a merge, and a rebuilt one doesn't, so the comparison is with the same index merged.
    memex_core::LexicalIndex::erase(r.s.lexical(), &r.v, &[]).unwrap();
    let before = hits(&r.s, &r.admin);
    assert!(before.iter().all(|h| !h.is_empty()));
    let unmerged_same = as_followed.iter().zip(&before).filter(|(a, b)| a == b).count();

    // Stop the server, delete the folder, start a new one on the same database and folders.
    let index_dir = TantivyLexical::new(&r.dir).vault_dir(&r.v);
    assert!(index_dir.exists());
    let (fresh, _pg) = server(&r.url, &r.dir);
    std::fs::remove_dir_all(&index_dir).unwrap();
    let admin = fresh.authenticate(&fresh.bootstrap("root2").unwrap().token).unwrap();
    // The registry is in memory, so the new server learns the vault from the old one's row.
    memex_core::Registry::insert_vault(
        fresh.registry(),
        &memex_types::VaultRow {
            id: r.v.clone(),
            name: "reindex".into(),
            created_by: "root".into(),
            created_at_ms: 0,
        },
        &memex_types::Audit {
            actor: "test".into(),
            time_ms: 0,
            action: "vault.create".into(),
            target: r.v.0.clone(),
        },
    )
    .unwrap();
    let started = Instant::now();
    let mut steps = 0;
    loop {
        steps += 1;
        let out = fresh.project_lexical(&r.v).unwrap();
        fresh.engine().clock().advance(Duration::from_secs(60));
        if out == StepOutcome::Idle
            && matches!(
                memex_core::LexicalIndex::health(fresh.lexical(), &r.v, false).unwrap(),
                memex_types::IndexHealth::Ready { .. }
            )
        {
            break;
        }
        assert!(steps < 10_000);
    }
    let rebuild_ms = started.elapsed().as_millis();
    let after = hits(&fresh, &admin);
    let segments = r.table(Table::Segment).len();
    println!(
        "rebuilt the keyword index of {n} documents, {segments} segments, in {rebuild_ms} ms over {steps} steps; {} of {} queries identical with every hit and score, {} hits in all; the index as followed, before its merge, matched the merged one on {unmerged_same}",
        after.iter().zip(&before).filter(|(a, b)| a == b).count(),
        queries.len(),
        after.iter().map(Vec::len).sum::<usize>()
    );
    let raw_order_same = after.iter().zip(&before).filter(|(a, b)| a == b).count();
    let (after, before) = (ordered(after), ordered(before));
    println!("{raw_order_same} of {} queries in exactly the same order before rounding", queries.len());
    for ((q, a), b) in queries.iter().zip(&after).zip(&before) {
        let differing: Vec<_> = (0..a.len().max(b.len()))
            .filter(|i| a.get(*i) != b.get(*i))
            .map(|i| (i, a.get(i), b.get(i)))
            .take(5)
            .collect();
        assert!(
            differing.is_empty(),
            "{q}: {} hits rebuilt, {} before; first differences {differing:?}",
            a.len(),
            b.len()
        );
    }
    let _ = &r.pg;
}
