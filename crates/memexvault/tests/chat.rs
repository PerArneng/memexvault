//! Chat on real Postgres, with a scripted model, so no LLM is called. Run with `MEMEX_TEST_DATABASE_URL` set, as in AGENTS.md. These say what the store and the server do, and nothing about a model.
// These tests read the environment and print what they measure.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::testing::{ChatCall, ChatStep, FakeEmbedder, FakeRasterizer, FakeReranker, ManualClock, MemBlobStore, MemRegistry, ScriptedChat};
use memex_core::{AgentRunner, ChatInput, ChatLimits, ChatSettingsUpdate, RENDITION_JOB, RenditionInput, SearchEdges, Server, ServerEdges, Store, UploadLimits, UploadRequest};
use memex_lexical::TantivyLexical;
use memex_store_postgres::{PgStore, schema_of_vault};
use memex_types::{ChatReservation, ChatThread, ChatThreadKey, ObjectId, VaultId};
use memex_vector::PgVectors;
use memexvault::edges::{PgVectorIndex, SystemClock, SystemSeed, TantivyIndex};
use memexvault::pg_edge::PostgresStore;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

struct Real;

impl ServerEdges for Real {
    type Caches = memex_core::NoCaches;
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
    url: String,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = self.s.delete_vault(&self.admin, &self.v);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rig(name: &str) -> Rig {
    let url = std::env::var("MEMEX_TEST_DATABASE_URL").expect("not run: MEMEX_TEST_DATABASE_URL isn't set");
    let pg = PgStore::connect(&url, 8).expect("connect");
    pg.migrate().expect("migrate");
    let pg = Arc::new(pg);
    let dir = std::env::temp_dir().join(format!("memex-chat-{name}-{}", std::process::id()));
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
            vector: PgVectorIndex(PgVectors::new(pg)),
            embedder: FakeEmbedder::default(),
            reranker: FakeReranker::default(),
        },
    ));
    s.set_chat(Some(ChatLimits::default()));
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, name).unwrap().id;
    s.set_chat_settings(
        &admin,
        &v,
        ChatSettingsUpdate {
            enabled: true,
            thread_token_budget: None,
            day_token_budget: None,
        },
    )
    .unwrap();
    Rig { s, admin, v, dir, url }
}

impl Rig {
    fn sql(&self) -> postgres::Client {
        postgres::Client::connect(&self.url, postgres::NoTls).expect("connect")
    }

    fn document(&self, name: &str, text: &str) -> (ObjectId, ObjectId) {
        let req = UploadRequest {
            name: name.into(),
            trust_tier: "trusted".into(),
            metadata: json!({}),
            replaces: None,
        };
        let source = self.s.upload(&self.admin, &self.v, req, UploadLimits::default(), &mut text.as_bytes()).unwrap().source.header.id;
        let job = self.s.claim_job(&self.v, RENDITION_JOB, "w").unwrap().unwrap();
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
        let rendition = self.s.list_renditions(&self.admin, &self.v, &source).unwrap().remove(0);
        let segment = self.s.list_segments(&self.admin, &self.v, &rendition.header.id, None, 10).unwrap().remove(0);
        (source, segment.header.id)
    }

    /// A whole run, on this thread, with the real tools.
    fn chat(&self, agent: &ScriptedChat, thread: &str, run: &str, message: &str) -> Vec<Value> {
        let prepared = self
            .s
            .prepare_chat(
                &self.admin,
                &self.v,
                ChatInput {
                    thread_id: thread.into(),
                    run_id: run.into(),
                    message: message.into(),
                },
                memex_core::new_chat_cancel(),
            )
            .unwrap();
        let sent = Arc::new(std::sync::Mutex::new(vec![]));
        let log = sent.clone();
        self.s.run_chat(prepared, agent, memex_api::chat_tool::<Real>, move |e| {
            log.lock().unwrap().push(serde_json::to_value(&e).unwrap());
            true
        });
        Arc::try_unwrap(sent).unwrap().into_inner().unwrap()
    }

    fn key(&self, thread: &str) -> ChatThreadKey {
        ChatThreadKey {
            owner: self.admin.principal.id.clone(),
            id: thread.into(),
        }
    }

    /// Every file behind the chat tables: each table, its TOAST table and TOAST index, and each index. Flushed first, so what's in memory is in them.
    fn chat_files(&self) -> Vec<(String, Vec<u8>)> {
        let mut c = self.sql();
        c.batch_execute("CHECKPOINT").unwrap();
        let schema = schema_of_vault(&self.v);
        let rows = c
            .query(
                "WITH owned AS (
                    SELECT t.oid, t.reltoastrelid FROM pg_class t JOIN pg_namespace n ON n.oid = t.relnamespace
                    WHERE n.nspname = $1 AND t.relname IN ('chat_threads', 'chat_messages', 'chat_thread_sources') AND t.relkind = 'r'
                 ), every AS (
                    SELECT oid FROM owned
                    UNION SELECT reltoastrelid FROM owned WHERE reltoastrelid <> 0
                    UNION SELECT indexrelid FROM pg_index WHERE indrelid IN (SELECT oid FROM owned)
                    UNION SELECT indexrelid FROM pg_index WHERE indrelid IN (SELECT reltoastrelid FROM owned WHERE reltoastrelid <> 0)
                 )
                 SELECT oid::regclass::text, pg_relation_filepath(oid) FROM every WHERE pg_relation_filepath(oid) IS NOT NULL",
                &[&schema],
            )
            .unwrap();
        rows.iter()
            .map(|r| {
                let (name, path): (String, String) = (r.get(0), r.get(1));
                let bytes: Vec<u8> = c.query_one("SELECT pg_read_binary_file($1, true)", &[&path]).unwrap().get::<_, Option<Vec<u8>>>(0).unwrap_or_default();
                (name, bytes)
            })
            .collect()
    }

    /// Which of the chat tables' files hold these bytes, read from the files and not through a query.
    fn raw_hits(&self, marker: &str) -> Vec<String> {
        let needle = marker.as_bytes();
        self.chat_files()
            .into_iter()
            .filter(|(_, bytes)| bytes.windows(needle.len()).any(|w| w == needle))
            .map(|(name, _)| name)
            .collect()
    }
}

/// Item 6 on real Postgres: the function spec 030 will call, then the store's physical removal, then a scan of the chat tables' own files for the markers. The positive control shows a marker is found in those files before deletion, so an empty result means something. It doesn't cover Postgres's WAL or anyone's backups, and spec 030 isn't built, so the cascade that would call this isn't wired.
#[test]
#[ignore = "needs Postgres"]
fn pg_erasing_a_source_leaves_no_marker_in_the_chat_tables_files() {
    let r = rig("chat-erase");
    let schema = schema_of_vault(&r.v);
    // Stored uncompressed here, so that a long message's marker can be seen in its TOAST file at all. The product's default compresses, which could only hide it from this scan.
    r.sql()
        .batch_execute(&format!("ALTER TABLE {schema}.chat_messages ALTER COLUMN message SET STORAGE EXTERNAL"))
        .unwrap();

    let (doc_mark, user_mark, reply_mark, title_mark) = ("ZQXSEGMENTMARKER", "ZQXUSERMARKER", "ZQXPARAPHRASEMARKER", "ZQXTITLEMARKER");
    let (long_mark, kept_mark) = ("ZQXLONGMARKER", "ZQXKEPTMARKER");
    let (source, segment) = r.document("target.txt", &format!("The vault holds {doc_mark} in its plain words."));
    let (_, other_segment) = r.document("other.txt", "Nothing about this document is secret.");

    // The reply never cites the source. It paraphrases what it read, and only the record of what reached the thread can find it.
    let long = format!("{long_mark} ").repeat(2_000);
    let agent = ScriptedChat::new(vec![
        vec![
            ChatCall::calls("read", json!({"kind": "segment", "id": segment.0})),
            ChatCall::says(&format!("Put another way, it says {reply_mark}, and no citation at all.")),
        ],
        vec![ChatCall::says(&long)],
        vec![ChatCall::calls("read", json!({"kind": "segment", "id": other_segment.0})), ChatCall::says(kept_mark)],
    ]);
    r.chat(&agent, "target", "r1", &format!("{title_mark} and {user_mark}: what does it say?"));
    r.chat(&agent, "target", "r2", "and at length?");
    r.chat(&agent, "kept", "r3", "the other one?");
    assert!(agent.tool_answers()[0].to_string().contains(doc_mark), "the tool's answer held the document's words");

    // The control: every marker is in the files now, found by reading them.
    let tables = |name: &str| format!("{schema}.{name}");
    for mark in [doc_mark, user_mark, reply_mark, title_mark, long_mark, kept_mark] {
        let hits = r.raw_hits(mark);
        println!("before erasing: {mark} in {hits:?}");
        assert!(!hits.is_empty(), "the control failed: {mark} isn't in any chat file, so this scan proves nothing");
    }
    assert!(r.raw_hits(title_mark).iter().any(|h| *h == tables("chat_threads")), "a title is on its own table");
    assert!(r.raw_hits(long_mark).iter().any(|h| h.starts_with("pg_toast")), "a long message is in TOAST, so it's covered too");

    // Chat isn't on the log, so nothing outlives a thread there.
    let mut c = r.sql();
    for mark in [user_mark, reply_mark, title_mark, long_mark, kept_mark] {
        let n: i64 = c
            .query_one(&format!("SELECT count(*) FROM {schema}.events WHERE payload::text LIKE '%' || $1 || '%'"), &[&mark])
            .unwrap()
            .get(0);
        assert_eq!(n, 0, "{mark} is in the log");
    }

    let erased = r.s.erase_chat_threads_citing(&r.v, &source).unwrap();
    assert_eq!(erased, vec![r.key("target")]);
    let lease = r.s.vault_lease_for_tests(&r.v).unwrap();
    r.s.engine().store().compact(&lease).unwrap();

    for mark in [doc_mark, user_mark, reply_mark, title_mark, long_mark] {
        let hits = r.raw_hits(mark);
        println!("after compacting: {mark} in {hits:?}");
        assert!(hits.is_empty(), "{mark} survived in {hits:?}");
        for table in ["chat_threads", "chat_messages", "chat_thread_sources"] {
            let n: i64 = c
                .query_one(&format!("SELECT count(*) FROM {schema}.{table} t WHERE t::text LIKE '%' || $1 || '%'"), &[&mark])
                .unwrap()
                .get(0);
            assert_eq!(n, 0, "{mark} in {table}");
        }
    }
    assert!(!r.raw_hits(kept_mark).is_empty(), "a thread that never read it stays");
    assert_eq!(r.s.get_chat_thread(&r.admin, &r.v, "kept", None, 10).unwrap().messages.len(), 3);
    assert!(r.s.get_chat_thread(&r.admin, &r.v, "target", None, 10).is_err());
    assert!(r.s.chat_day_spend(&r.admin, &r.v).unwrap().0 > 0, "and the day's spending stays");
}

#[test]
#[ignore = "needs Postgres"]
fn pg_plain_vacuum_alone_is_not_enough_which_is_why_compact_rewrites() {
    // What compact does differently and why: this is the same marker test with plain VACUUM. A page that still holds other rows keeps the dead bytes, so it's a check on the reason, and it says so if a Postgres version changes that.
    let r = rig("chat-vacuum");
    let schema = schema_of_vault(&r.v);
    let keep = ScriptedChat::new(vec![vec![ChatCall::says("a row that stays on the page")], vec![ChatCall::says("ZQXDEADBYTES")]]);
    r.chat(&keep, "kept", "r1", "first");
    r.chat(&keep, "gone", "r2", "second");
    assert!(!r.raw_hits("ZQXDEADBYTES").is_empty());
    assert!(r.s.engine().store().chat_delete_thread(&r.v, &r.key("gone")).unwrap());
    r.sql().batch_execute(&format!("VACUUM {schema}.chat_messages; VACUUM {schema}.chat_threads")).unwrap();
    println!("after plain VACUUM the bytes are in: {:?}", r.raw_hits("ZQXDEADBYTES"));
    let lease = r.s.vault_lease_for_tests(&r.v).unwrap();
    r.s.engine().store().compact(&lease).unwrap();
    assert!(r.raw_hits("ZQXDEADBYTES").is_empty(), "compact removes them");
}

/// Reserving is what keeps concurrent runs inside a budget. Eight threads race for what only five can have.
#[test]
#[ignore = "needs Postgres"]
fn pg_concurrent_reservations_cannot_overspend_a_budget() {
    let r = rig("chat-race");
    let store = r.s.engine().store();
    let threads: Vec<ChatThreadKey> = (0..8).map(|n| r.key(&format!("t{n}"))).collect();
    for key in &threads {
        store
            .chat_create_thread(
                &r.v,
                &ChatThread {
                    owner: key.owner.clone(),
                    id: key.id.clone(),
                    title: "t".into(),
                    created_at_ms: 1,
                    updated_at_ms: 1,
                    tokens: 0,
                },
            )
            .unwrap();
    }
    let won = AtomicU32::new(0);
    std::thread::scope(|scope| {
        for key in &threads {
            let (s, v, won) = (r.s.clone(), r.v.clone(), &won);
            scope.spawn(move || {
                for _ in 0..10 {
                    if s.engine().store().chat_reserve(&v, key, 99, 10, 1000, 50).unwrap() == ChatReservation::Reserved {
                        won.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        }
    });
    assert_eq!(won.load(Ordering::SeqCst), 5);
    assert_eq!(store.chat_day_tokens(&r.v, 99).unwrap(), 50);
}

/// A run holds no connection or transaction while the model works, however many are waiting. The server's limit is 64 and `PgStore` doesn't bound its own, so a run per connection would be a real risk.
#[test]
#[ignore = "needs Postgres"]
fn pg_runs_waiting_on_a_model_hold_no_connection_or_transaction() {
    let r = rig("chat-conns");
    r.s.set_chat(Some(ChatLimits {
        max_runs: 8,
        ..ChatLimits::default()
    }));
    let mut c = r.sql();
    let database: String = c.query_one("SELECT current_database()", &[]).unwrap().get(0);
    let count = |c: &mut postgres::Client, condition: &str| -> i64 {
        c.query_one(
            &format!("SELECT count(*) FROM pg_stat_activity WHERE datname = $1 AND pid <> pg_backend_pid() AND {condition}"),
            &[&database],
        )
        .unwrap()
        .get(0)
    };
    let before = count(&mut c, "true");
    let mut waiting = vec![];
    let mut flags = vec![];
    for n in 0..8 {
        let cancel = memex_core::new_chat_cancel();
        flags.push(cancel.clone());
        let prepared = r
            .s
            .prepare_chat(
                &r.admin,
                &r.v,
                ChatInput {
                    thread_id: format!("t{n}"),
                    run_id: format!("r{n}"),
                    message: "hi".into(),
                },
                cancel,
            )
            .unwrap();
        let s = r.s.clone();
        waiting.push(std::thread::spawn(move || {
            let agent = ScriptedChat::new(vec![vec![ChatCall::new(vec![ChatStep::Text("thinking".into()), ChatStep::UntilCancelled])]]);
            s.run_chat(prepared, &agent, memex_api::chat_tool::<Real>, |_| true);
            agent.limits().len()
        }));
    }
    // A ninth is refused rather than queued, which is the bound.
    let ninth = r.s.prepare_chat(
        &r.admin,
        &r.v,
        ChatInput {
            thread_id: "t9".into(),
            run_id: "r9".into(),
            message: "hi".into(),
        },
        memex_core::new_chat_cancel(),
    );
    assert!(matches!(ninth, Err(memex_core::ServerError::Busy)));
    // Wait until all eight are in their model call.
    let started = Instant::now();
    loop {
        let spent = r.s.chat_day_spend(&r.admin, &r.v).unwrap().0;
        if spent >= 8 * (100 + u64::from(ChatLimits::default().output_tokens)) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "the runs didn't start: {spent}");
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(200));
    let (total, busy, in_transaction) = (
        count(&mut c, "true"),
        count(&mut c, "state = 'active'"),
        count(&mut c, "state LIKE 'idle in transaction%'"),
    );
    println!("8 runs in a model call: {before} connections before, {total} during, {busy} active, {in_transaction} in a transaction");
    assert_eq!(busy, 0, "no statement is running");
    assert_eq!(in_transaction, 0, "no snapshot or transaction is open");
    assert!(total <= before + 8, "connections stay within the pool's bound of 8: {before} to {total}");
    for flag in &flags {
        flag.store(true, Ordering::SeqCst);
    }
    for w in waiting {
        assert_eq!(w.join().unwrap(), 1);
    }
}

#[test]
#[ignore = "needs Postgres"]
fn pg_a_run_keeps_its_thread_its_sources_and_its_derivation_and_a_proposal_waits() {
    let r = rig("chat-run");
    let quote = "The bridge is 412 metres long.";
    let (source, segment) = r.document("d.txt", quote);
    r.s.record_audit_samples(&r.v, memex_core::chat::CHAT_AGENT, &memex_core::chat::prompt_hash(), "claim", 500, 0, "test", "plenty").unwrap();
    let op = json!({"op": "add_assertion", "id": "c1", "subject": "bridge", "predicate": "length", "value": quote, "epistemic": "asserted", "machine": false, "confidence_permille": 900, "valid_from_ms": null, "valid_to_ms": null,
        "evidence": [["c1-e", {"segment_id": segment.0, "quote": quote, "stance": "supports"}]]});
    let agent = ScriptedChat::new(vec![vec![
        ChatCall::calls("read", json!({"kind": "segment", "id": segment.0})),
        ChatCall::calls("propose", json!({"ops": [op], "rationale": "it says so"})),
        ChatCall::says("Queued for a person."),
    ]]);
    let frames = r.chat(&agent, "t", "r", "add it");
    assert_eq!(frames.last().unwrap()["type"], json!("RUN_FINISHED"));
    let answers = agent.tool_answers();
    assert_eq!(answers[1]["data"]["state"], json!("pending"), "{}", answers[1]);
    let id = ObjectId(answers[1]["data"]["proposal_id"].as_str().unwrap().into());
    let cs = r.s.get_changeset(&r.admin, &r.v, &id).unwrap();
    assert_eq!((cs.proposer.as_str(), cs.requested_by.as_deref()), (memex_core::chat::CHAT_AGENT, Some(r.admin.principal.id.as_str())));
    assert_eq!(r.s.engine().store().chat_sources(&r.v, &r.key("t")).unwrap(), vec![source]);
    let view = r.s.get_chat_thread(&r.admin, &r.v, "t", None, 10).unwrap();
    assert_eq!(view.messages.len(), 4, "the question, two tool answers and the reply");
    assert!(view.thread.tokens >= 300);
    assert_eq!(r.s.chat_day_spend(&r.admin, &r.v).unwrap().0, 300);
    let approved = r.s.approve(&r.admin, &r.v, &id, None).unwrap();
    assert_eq!(approved.requested_by.as_deref(), Some(r.admin.principal.id.as_str()));
    assert_eq!(agent.limits().len(), 3);
}

/// A generic bound on what it takes to keep the compiler honest about the trait above, as `ScriptedChat` has to stay shareable.
#[allow(dead_code)]
fn is_shared<A: AgentRunner + Send + Sync>() {}
const _: fn() = is_shared::<ScriptedChat>;
