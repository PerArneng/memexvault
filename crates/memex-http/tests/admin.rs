//! Spec 039 in memory: operations as jobs with progress and reports, imports, settings with secrets left out, status with the LLM down, the usage report and the scope matrix. Fake edges, real routing and JSON.
// The status test times the route and prints what it measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
use memex_core::{ArchiveStore, NewToken, OperationEdge, Server, ServerEdges, ServerError, Settings, StateRecorded};
use memex_http::{AdminConfig, admin_routes, embedded_asset, router_with};
use memex_types::{
    AgentUsage, DerivationOutcome, DerivationRow, EventDraft, Grant, Header, Mutation, OPERATION_JOB, ObjectId, PrincipalKind, Row, Scope, Status, VaultId,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::ServiceExt;

struct Fakes;

impl ServerEdges for Fakes {
    type Registry = MemRegistry;
    type Store = MemStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = ManualClock;
    type Seed = SeededRandom;
    type Rasterizer = memex_core::testing::FakeRasterizer;
    type Lexical = memex_core::testing::MemLexical;
    type Vector = memex_core::testing::MemVector;
    type Embedder = memex_core::testing::FakeEmbedder;
    type Reranker = memex_core::testing::FakeReranker;
    type Caches = memex_core::NoCaches;
}

#[derive(Default)]
struct MemArchives(Mutex<HashMap<String, Vec<u8>>>);

impl ArchiveStore for MemArchives {
    fn put(&self, name: &str, write: &mut dyn FnMut(&mut dyn Write) -> Result<(), ServerError>) -> Result<u64, ServerError> {
        let mut buf = vec![];
        write(&mut buf)?;
        let n = buf.len() as u64;
        self.0.lock().unwrap().insert(name.into(), buf);
        Ok(n)
    }
    fn open(&self, name: &str) -> Result<Option<memex_core::ArchiveRead>, ServerError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(name)
            .map(|b| (b.len() as u64, Box::new(std::io::Cursor::new(b.clone())) as Box<dyn Read + Send>)))
    }
    fn remove(&self, name: &str) -> Result<(), ServerError> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}

const MARKER: &str = "ZQXSECRETMARKER9931";

struct World {
    s: Arc<Server<Fakes>>,
    admin: String,
    a: VaultId,
    b: VaultId,
    archives: Arc<MemArchives>,
    settings: Settings,
    llm_endpoint: Option<String>,
    /// One router, so imports, held in its memory, are there on the next request. Cleared when the config changes.
    app: Mutex<Option<axum::Router>>,
}

fn settings_with_secrets() -> Settings {
    let vars: HashMap<&str, String> = HashMap::from([
        (
            "MEMEX_DATABASE_URL",
            format!("postgres://memex:{MARKER}@db.example:5432/memex?password={MARKER}"),
        ),
        ("MEMEX_LLM_PROVIDER", "openai".into()),
        ("MEMEX_LLM_MODEL", "m1".into()),
        ("MEMEX_LLM_BASE_URL", format!("http://user:{MARKER}@127.0.0.1:1/v1")),
        ("MEMEX_LLM_API_KEY", MARKER.into()),
        ("MEMEX_BACKEND_TRANSCRIBER", "gemini".into()),
        ("MEMEX_TRANSCRIBE_API_MODEL", "t1".into()),
        ("MEMEX_TRANSCRIBE_API_KEY", MARKER.into()),
        ("MEMEX_MCP_TOKEN", MARKER.into()),
    ]);
    Settings::from_vars(|n| vars.get(n).cloned()).expect("settings")
}

fn world() -> World {
    let s = Arc::new(Server::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_700_000_000_000),
        SeededRandom::new(11),
        memex_core::testing::mem_search(),
    ));
    let admin = s.bootstrap("root").unwrap().token;
    let c = s.authenticate(&admin).unwrap();
    let a = s.create_vault(&c, "vault-a").unwrap().id;
    let b = s.create_vault(&c, "vault-b").unwrap().id;
    World {
        s,
        admin,
        a,
        b,
        archives: Arc::new(MemArchives::default()),
        settings: settings_with_secrets(),
        llm_endpoint: None,
        app: Mutex::new(None),
    }
}

fn config(w: &World, prices: Vec<memex_core::ModelPrice>) -> AdminConfig<MemArchives> {
    AdminConfig {
        archives: w.archives.clone(),
        settings: w.settings.effective(),
        prices,
        llm_endpoint: w.llm_endpoint.clone(),
        models: vec![],
        mcp: memex_api::McpInfoDto {
            endpoint: "/api/v1/mcp".into(),
            transports: vec!["streamable-http".into()],
            protocol_version: "2026-07-28".into(),
            supported_protocol_versions: vec![],
            allowed_hosts: w.settings.mcp_allowed_hosts.clone(),
            tools: vec![],
        },
        memory: || Some(1),
        abandoned: || 0,
    }
}

async fn call_with(w: &World, prices: Vec<memex_core::ModelPrice>, method: &str, path: &str, token: Option<&str>, body: Option<Body>) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(b).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let app = if prices.is_empty() {
        let mut cached = w.app.lock().unwrap();
        cached
            .get_or_insert_with(|| router_with(w.s.clone(), embedded_asset, admin_routes(config(w, vec![]))))
            .clone()
    } else {
        router_with(w.s.clone(), embedded_asset, admin_routes(config(w, prices)))
    };
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    (status, to_bytes(res.into_body(), 64 << 20).await.unwrap().to_vec())
}

async fn call(w: &World, method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let (status, bytes) = call_with(w, vec![], method, path, token, body.map(|b| Body::from(b.to_string()))).await;
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn token_with(w: &World, name: &str, scope: Option<Scope>) -> String {
    let c = w.s.authenticate(&w.admin).unwrap();
    let p = w.s.create_principal(&c, name, PrincipalKind::Agent, false).unwrap();
    let grants = scope.map(|sc| Grant { vault: w.a.clone(), scope: sc }).into_iter().collect();
    w.s.create_token(
        &c,
        NewToken {
            principal_id: Some(p.id),
            name: "t".into(),
            grants,
            expires_at_ms: None,
        },
    )
    .unwrap()
    .token
}

/// Runs the vault's queued operations as a worker would, stepping the projectors when a job waits for a build.
fn run_operations(w: &World, vault: &VaultId) -> usize {
    struct Edge<'a> {
        s: &'a Server<Fakes>,
        v: &'a VaultId,
        archives: &'a MemArchives,
    }
    impl OperationEdge for Edge<'_> {
        fn pause(&self, _: Duration) {
            let _ = self.s.project_lexical(self.v);
            let _ = self.s.project_vector(self.v);
        }
        fn lost(&self) -> bool {
            false
        }
        fn archives(&self) -> &dyn ArchiveStore {
            self.archives
        }
    }
    let edge = Edge {
        s: &w.s,
        v: vault,
        archives: &w.archives,
    };
    let mut n = 0;
    while let Some(job) = w.s.claim_job(vault, OPERATION_JOB, "w").unwrap() {
        match w.s.run_operation_job(vault, &job, &edge) {
            Ok(()) => assert!(w.s.complete_job(vault, &job, vec![]).unwrap()),
            Err(e) => assert!(w.s.fail_job(vault, &job, &format!("{e:?}"), false).unwrap()),
        }
        n += 1;
    }
    n
}

fn settle(w: &World, vault: &VaultId) {
    for _ in 0..50 {
        let _ = w.s.project_lexical(vault);
        let _ = w.s.project_vector(vault);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_admin_route_needs_admin_scope() {
    use StatusCode as S;
    let w = world();
    let (a, b) = (w.a.0.clone(), w.b.0.clone());
    let roles = [Some(Scope::Read), Some(Scope::Propose), Some(Scope::Admin), None];
    let tokens: Vec<String> = roles.iter().enumerate().map(|(i, r)| token_with(&w, &format!("role{i}"), *r)).collect();
    // An export to have something to download and a job to read.
    let (st, job) = call(
        &w,
        "POST",
        &format!("/api/v1/vaults/{a}/operations"),
        Some(&w.admin),
        Some(json!({"op": "export"})),
    )
    .await;
    assert_eq!(st, S::ACCEPTED, "{job}");
    run_operations(&w, &w.a);
    let jid = job["id"].as_str().unwrap().to_string();
    // For read, propose, admin in vault A, and no grants. Vault B is no one's but the server admin's.
    let cases: Vec<(&str, String, Option<Value>, [S; 4])> = vec![
        (
            "POST",
            format!("/api/v1/vaults/{a}/operations"),
            Some(json!({"op": "compact"})),
            [S::FORBIDDEN, S::FORBIDDEN, S::ACCEPTED, S::NOT_FOUND],
        ),
        (
            "POST",
            format!("/api/v1/vaults/{b}/operations"),
            Some(json!({"op": "compact"})),
            [S::NOT_FOUND; 4],
        ),
        (
            "GET",
            format!("/api/v1/vaults/{a}/operations"),
            None,
            [S::FORBIDDEN, S::FORBIDDEN, S::OK, S::NOT_FOUND],
        ),
        (
            "GET",
            format!("/api/v1/vaults/{a}/operations/{jid}"),
            None,
            [S::FORBIDDEN, S::FORBIDDEN, S::OK, S::NOT_FOUND],
        ),
        (
            "GET",
            format!("/api/v1/vaults/{a}/operations/{jid}/archive"),
            None,
            [S::FORBIDDEN, S::FORBIDDEN, S::OK, S::NOT_FOUND],
        ),
        ("GET", format!("/api/v1/vaults/{b}/operations"), None, [S::NOT_FOUND; 4]),
        ("POST", "/api/v1/imports".into(), Some(json!({})), [S::FORBIDDEN; 4]),
        ("GET", "/api/v1/imports/nothing".into(), None, [S::FORBIDDEN; 4]),
        ("GET", "/api/v1/system/settings".into(), None, [S::FORBIDDEN; 4]),
        ("GET", "/api/v1/system/status".into(), None, [S::FORBIDDEN; 4]),
        ("GET", "/api/v1/system/usage".into(), None, [S::FORBIDDEN; 4]),
        ("GET", "/api/v1/system/mcp".into(), None, [S::FORBIDDEN; 4]),
        ("POST", "/api/v1/caches/clear".into(), None, [S::FORBIDDEN; 4]),
    ];
    for (method, path, body, expected) in &cases {
        for (i, token) in tokens.iter().enumerate() {
            let before = w.s.registry().fingerprint();
            let (status, out) = call(&w, method, path, Some(token), body.clone()).await;
            assert_eq!(status, expected[i], "{method} {path} role {i}: {out}");
            if !status.is_success() {
                assert_eq!(w.s.registry().fingerprint(), before, "{method} {path} role {i} changed state");
            }
        }
        let (status, _) = call(&w, method, path, None, body.clone()).await;
        assert_eq!(status, S::UNAUTHORIZED, "{method} {path} without a token");
    }
    // Refused imports leave no archive behind.
    assert!(w.archives.0.lock().unwrap().keys().all(|k| k.starts_with("exports/")));
    // The server admin passes every one of them.
    for path in ["/api/v1/system/settings", "/api/v1/system/status", "/api/v1/system/usage", "/api/v1/system/mcp"] {
        assert_eq!(call(&w, "GET", path, Some(&w.admin), None).await.0, S::OK, "{path}");
    }
    assert_eq!(call(&w, "POST", "/api/v1/caches/clear", Some(&w.admin), None).await.0, S::OK);
    assert_eq!(call(&w, "GET", &format!("/api/v1/vaults/{b}/operations"), Some(&w.admin), None).await.0, S::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn operations_report_progress_in_the_jobs_api_and_match_the_commands() {
    let w = world();
    let (a, v) = (w.a.0.clone(), w.a.clone());
    let admin = w.s.authenticate(&w.admin).unwrap();
    let mut text: &[u8] = b"Plain words about the harbour and its ferries.";
    w.s.upload(
        &admin,
        &v,
        memex_core::UploadRequest {
            name: "notes.txt".into(),
            trust_tier: "unverified".into(),
            metadata: json!({}),
            replaces: None,
        },
        memex_core::UploadLimits::default(),
        &mut text,
    )
    .unwrap();
    settle(&w, &v);
    let base = format!("/api/v1/vaults/{a}");
    let start = |body: Value| {
        let w = &w;
        let base = base.clone();
        async move { call(w, "POST", &format!("{base}/operations"), Some(&w.admin), Some(body)).await }
    };

    // Reindex: a second of the same kind is refused while the first is queued, naming it. Another index isn't.
    let (st, first) = start(json!({"op": "reindex", "index": "keyword"})).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{first}");
    assert_eq!((first["kind"].as_str(), first["state"].as_str()), (Some("operation"), Some("queued")));
    let (st, again) = start(json!({"op": "reindex", "index": "keyword"})).await;
    assert_eq!(st, StatusCode::CONFLICT, "{again}");
    let message = again["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("keyword reindex is already queued") && message.contains(first["id"].as_str().unwrap()),
        "{message}"
    );
    let (st, rollback) = start(json!({"op": "rollback", "index": "keyword"})).await;
    assert_eq!(st, StatusCode::CONFLICT, "a rollback during a reindex of the same index: {rollback}");
    let (st, vector) = start(json!({"op": "reindex", "index": "vector"})).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{vector}");
    let (st, bad) = start(json!({"op": "reindex", "index": "both"})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
    let (st, bad) = start(json!({"op": "defragment"})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
    assert_eq!(run_operations(&w, &v), 2);
    let id = first["id"].as_str().unwrap();
    // The jobs API shows the same progress and report as the operation's own route.
    let (_, job) = call(&w, "GET", &format!("{base}/jobs/{id}"), Some(&w.admin), None).await;
    let (_, op) = call(&w, "GET", &format!("{base}/operations/{id}"), Some(&w.admin), None).await;
    assert_eq!(job, op);
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(job["progress"], json!({"step": 2, "of": 2, "label": "the new generation is live"}));
    assert_eq!(job["report"]["index"], "keyword");
    assert!(job["report"]["live_watermark"].is_u64(), "{job}");
    // Now it's done, a reindex may start again, and so may a rollback, to the generation before.
    let (st, rollback) = start(json!({"op": "rollback", "index": "keyword"})).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{rollback}");
    run_operations(&w, &v);
    let (_, rolled) = call(
        &w,
        "GET",
        &format!("{base}/operations/{}", rollback["id"].as_str().unwrap()),
        Some(&w.admin),
        None,
    )
    .await;
    assert_eq!(rolled["state"], "done", "{rolled}");
    settle(&w, &v);

    // Reconcile gives what the command's call gives.
    let (_, rec) = start(json!({"op": "reconcile", "deep": true})).await;
    run_operations(&w, &v);
    let (_, rec) = call(&w, "GET", &format!("{base}/operations/{}", rec["id"].as_str().unwrap()), Some(&w.admin), None).await;
    // The job's own completion comes after its report, so the command compares at the report's head: the log replayed there is the tables the job read.
    let report = &rec["report"];
    let head = report["head"].as_u64().unwrap();
    let cli = w.s.reconcile_store(&v, Some(head), true).unwrap();
    assert_eq!(report["tables_sha256"], cli.log.at.as_ref().unwrap().1.as_str());
    assert_eq!(report["replayed_sha256"], report["tables_sha256"]);
    assert_eq!(cli.log.tables_sha256, cli.log.replayed_sha256, "the command finds it clean too");
    assert_eq!(report["blobs_stored"], cli.blobs.stored);
    assert_eq!(report["blobs_referenced"], cli.blobs.referenced);
    assert_eq!(report["clean"], true, "{report}");
    assert_eq!(report["indexes"].as_array().unwrap().len(), 4);

    // Export writes the same bytes as `memexvault backup`, and the archive route serves them.
    let (_, exp) = start(json!({"op": "export"})).await;
    let eid = exp["id"].as_str().unwrap().to_string();
    let (st, early) = call(&w, "GET", &format!("{base}/operations/{eid}/archive"), Some(&w.admin), None).await;
    assert_eq!(st, StatusCode::CONFLICT, "not before it's done: {early}");
    run_operations(&w, &v);
    let (st, archive) = call_with(&w, vec![], "GET", &format!("{base}/operations/{eid}/archive"), Some(&w.admin), None).await;
    assert_eq!(st, StatusCode::OK);
    // The command, run now, sees the export's own completion too, so they meet at the export's head: the archive holds the tables as they were there.
    let (header, summary) = memex_core::backup::verify(&mut archive.as_slice()).unwrap();
    let at = w.s.reconcile_store(&v, header.head, false).unwrap();
    assert_eq!(header.dump_sha256, at.log.at.as_ref().unwrap().1, "the export holds the vault at its head");
    let mut cli = vec![];
    let (now_header, now_summary) = w.s.backup_vault(&v, &mut cli).unwrap();
    assert_eq!(now_summary.blobs, summary.blobs);
    assert_eq!(now_summary.events, summary.events + 2, "the export's last progress and its completion");
    assert_eq!(now_header.dump_sha256, at.log.tables_sha256);
    let (_, exp) = call(&w, "GET", &format!("{base}/operations/{eid}"), Some(&w.admin), None).await;
    assert_eq!(exp["report"]["dump_sha256"], header.dump_sha256.as_str());
    assert_eq!(exp["report"]["events"], summary.events);
    assert_eq!(exp["report"]["bytes"], archive.len() as u64);
    let (st, _) = call(
        &w,
        "GET",
        &format!("{base}/operations/{}/archive", rec["id"].as_str().unwrap()),
        Some(&w.admin),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "a reconcile has no archive");

    // Sweep and compact report too.
    let (_, sw) = start(json!({"op": "sweep", "grace_s": 0})).await;
    let (_, co) = start(json!({"op": "compact"})).await;
    run_operations(&w, &v);
    let (_, sw) = call(&w, "GET", &format!("{base}/operations/{}", sw["id"].as_str().unwrap()), Some(&w.admin), None).await;
    assert_eq!((sw["state"].as_str(), sw["report"]["removed"].as_u64()), (Some("done"), Some(0)), "{sw}");
    assert!(sw["report"]["checked"].as_u64().unwrap() >= 1);
    let (_, co) = call(&w, "GET", &format!("{base}/operations/{}", co["id"].as_str().unwrap()), Some(&w.admin), None).await;
    assert_eq!(co["report"]["compacted"], true, "{co}");
    let (_, list) = call(&w, "GET", &format!("{base}/operations?limit=100"), Some(&w.admin), None).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 7);
    assert!(list["items"].as_array().unwrap().iter().all(|j| j["state"] == "done"), "{list}");

    // Import: over an existing vault only with the flag, refused at once without it, and under a new id it comes out the same.
    let upload = |query: &'static str| {
        let (w, bytes) = (&w, archive.clone());
        async move {
            let (st, out) = call_with(w, vec![], "POST", &format!("/api/v1/imports{query}"), Some(&w.admin), Some(Body::from(bytes))).await;
            (st, serde_json::from_slice::<Value>(&out).unwrap_or(Value::Null))
        }
    };
    let (st, refused) = upload("").await;
    assert_eq!(st, StatusCode::CONFLICT, "{refused}");
    assert!(
        refused["error"]["message"].as_str().unwrap().contains("only when told to replace it"),
        "{refused}"
    );
    assert!(
        w.archives.0.lock().unwrap().keys().all(|k| !k.starts_with("imports/")),
        "a refused import leaves nothing"
    );
    let (st, both) = upload("?new_id=true&replace=true").await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{both}");
    let (st, damaged) = call_with(
        &w,
        vec![],
        "POST",
        "/api/v1/imports?new_id=true",
        Some(&w.admin),
        Some(Body::from(archive[..archive.len() / 2].to_vec())),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::ACCEPTED,
        "the header reads, so it starts: {}",
        String::from_utf8_lossy(&damaged)
    );
    let damaged: Value = serde_json::from_slice(&damaged).unwrap();
    let wait_import = |id: String| {
        let w = &w;
        async move {
            for _ in 0..200 {
                let (_, imp) = call(w, "GET", &format!("/api/v1/imports/{id}"), Some(&w.admin), None).await;
                if imp["state"] != "running" {
                    return imp;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("the import didn't finish");
        }
    };
    let failed = wait_import(damaged["id"].as_str().unwrap().into()).await;
    assert_eq!(failed["state"], "failed", "{failed}");
    let vaults_before = w.s.vault_ids().unwrap().len();
    let (st, started) = upload("?new_id=true&name=copy").await;
    assert_eq!(st, StatusCode::ACCEPTED, "{started}");
    assert_eq!(started["progress"]["step"], 1);
    let done = wait_import(started["id"].as_str().unwrap().into()).await;
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(w.s.vault_ids().unwrap().len(), vaults_before + 1);
    let copy = VaultId(done["report"]["vault_id"].as_str().unwrap().into());
    assert_eq!(done["report"]["name"], "copy");
    assert_eq!(done["report"]["events"], summary.events);
    let mut again = vec![];
    let (copied, _) = w.s.backup_vault(&copy, &mut again).unwrap();
    assert_eq!(copied.dump_sha256, header.dump_sha256, "the restored tables are the exported ones");
    let (st, replaced) = upload("?replace=true").await;
    assert_eq!(st, StatusCode::ACCEPTED, "{replaced}");
    let replaced = wait_import(replaced["id"].as_str().unwrap().into()).await;
    assert_eq!(
        (replaced["state"].as_str(), replaced["report"]["vault_id"].as_str()),
        (Some("done"), Some(a.as_str())),
        "{replaced}"
    );
    assert!(
        w.archives.0.lock().unwrap().keys().all(|k| !k.starts_with("imports/")),
        "finished imports leave nothing"
    );
}

fn derivation(id: &str, kind: &str, model: &str, at: i64, i: u64, o: u64, t: u64) -> DerivationRow {
    DerivationRow {
        header: Header {
            id: ObjectId(id.into()),
            creator: "extractor".into(),
            status: Status::Active,
            version: 1,
            acl_id: None,
        },
        kind: kind.into(),
        job_id: ObjectId("j".into()),
        source_id: ObjectId("s".into()),
        rendition_id: ObjectId("r".into()),
        segment_ids: vec![],
        model: model.into(),
        prompt_hash: "p".into(),
        usage: AgentUsage {
            input_tokens: i,
            output_tokens: o,
            thinking_tokens: t,
        },
        attempts: 1,
        outcome: DerivationOutcome::Done,
        detail: None,
        items: vec![],
        rejected: vec![],
        created_at_ms: at,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_usage_report_matches_the_derivations_and_shows_no_cost_without_a_price() {
    let w = world();
    let day = 86_400_000;
    let t0 = 1_700_000_000_000;
    let mut rows = vec![];
    for (n, (vault, kind, model, at)) in [
        (&w.a, "extraction", "m1", t0),
        (&w.a, "extraction", "m1", t0 + 1000),
        (&w.a, "chat", "m2", t0),
        (&w.b, "extraction", "m1", t0),
        (&w.a, "extraction", "m1", t0 + 2 * day),
    ]
    .into_iter()
    .enumerate()
    {
        let n = n as u64;
        let d = derivation(&format!("d{n}"), kind, model, at, 100 + n, 10 * n, n);
        rows.push((vault.clone(), d.clone()));
        let lease = w.s.vault_lease_for_tests(vault).unwrap();
        w.s.engine()
            .apply(
                &lease,
                EventDraft {
                    actor: "extractor".into(),
                    kind: "state.recorded".into(),
                    schema_version: 1,
                    payload: serde_json::to_value(StateRecorded {
                        mutations: vec![Mutation::Put(Row::Derivation(d))],
                    })
                    .unwrap(),
                },
            )
            .unwrap();
    }
    let (st, report) = call(&w, "GET", "/api/v1/system/usage", Some(&w.admin), None).await;
    assert_eq!(st, StatusCode::OK, "{report}");
    let report_rows = report["rows"].as_array().unwrap();
    assert_eq!(report_rows.len(), 4, "{report}");
    assert!(report_rows.iter().all(|r| r["cost_usd"].is_null()), "no price, no cost: {report}");
    assert_eq!(report["priced_cost_usd"], 0.0);
    // The same sums, computed here over the rows.
    for r in report_rows {
        let matching: Vec<&DerivationRow> = rows
            .iter()
            .filter(|(v, d)| {
                v.0 == r["vault_id"] && d.kind == r["agent"] && d.model == r["model"] && memex_core::utc_day(d.created_at_ms) == r["day"].as_str().unwrap()
            })
            .map(|(_, d)| d)
            .collect();
        assert_eq!(r["derivations"], matching.len() as u64);
        assert_eq!(r["input_tokens"], matching.iter().map(|d| d.usage.input_tokens).sum::<u64>());
        assert_eq!(r["output_tokens"], matching.iter().map(|d| d.usage.output_tokens).sum::<u64>());
        assert_eq!(r["thinking_tokens"], matching.iter().map(|d| d.usage.thinking_tokens).sum::<u64>());
    }
    let total: u64 = rows.iter().map(|(_, d)| d.usage.total()).sum();
    assert_eq!(report["unpriced_tokens"], total);
    // A day's window, inclusive.
    let (_, one) = call(&w, "GET", "/api/v1/system/usage?from=2023-11-14&to=2023-11-14", Some(&w.admin), None).await;
    assert_eq!(one["rows"].as_array().unwrap().len(), 3, "{one}");
    let (st, bad) = call(&w, "GET", "/api/v1/system/usage?from=2023-02-30", Some(&w.admin), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
    // With a price for m1 only, m1's rows have a cost and m2's still don't.
    let prices = memex_core::parse_prices("m1=2:8:8").unwrap();
    let (_, bytes) = call_with(&w, prices, "GET", "/api/v1/system/usage", Some(&w.admin), None).await;
    let priced: Value = serde_json::from_slice(&bytes).unwrap();
    let mut expected = 0.0;
    for r in priced["rows"].as_array().unwrap() {
        if r["model"] == "m1" {
            let c = (r["input_tokens"].as_f64().unwrap() * 2.0 + (r["output_tokens"].as_f64().unwrap() + r["thinking_tokens"].as_f64().unwrap()) * 8.0) / 1e6;
            assert!((r["cost_usd"].as_f64().unwrap() - c).abs() < 1e-12, "{r}");
            expected += c;
        } else {
            assert!(r["cost_usd"].is_null(), "{r}");
        }
    }
    assert!((priced["priced_cost_usd"].as_f64().unwrap() - expected).abs() < 1e-12);
}

#[tokio::test(flavor = "multi_thread")]
async fn secrets_appear_in_no_settings_status_or_error_response() {
    let mut w = world();
    w.llm_endpoint = w.settings.llm.as_ref().and_then(memex_core::llm_probe_target);
    assert_eq!(w.llm_endpoint.as_deref(), Some("127.0.0.1:1"), "the probe target drops the user and password");
    let a = w.a.0.clone();
    let mut bodies = vec![];
    for (method, path, body) in [
        ("GET", "/api/v1/system/settings".to_string(), None),
        ("GET", "/api/v1/system/status".into(), None),
        ("GET", "/api/v1/system/mcp".into(), None),
        ("GET", "/api/v1/system/usage?from=nope".into(), None),
        ("GET", "/api/v1/imports/missing".into(), None),
        ("POST", format!("/api/v1/vaults/{a}/operations"), Some(json!({"op": "nope"}))),
        ("POST", "/api/v1/imports".into(), Some(json!({"not": "an archive"}))),
        ("GET", "/api/v1/vaults/missing/operations".into(), None),
    ] {
        let (status, out) = call(&w, method, &path, Some(&w.admin), body).await;
        assert!(status != StatusCode::INTERNAL_SERVER_ERROR, "{path}: {out}");
        bodies.push((path, out.to_string()));
    }
    for (path, out) in &bodies {
        assert!(!out.contains(MARKER), "{path} shows a secret: {out}");
    }
    let settings: Value = serde_json::from_str(&bodies[0].1).unwrap();
    assert_eq!(settings["database_url"], "postgres://<redacted>@db.example:5432/memex");
    assert_eq!(settings["llm"]["api_key"], "<set>");
    assert_eq!(settings["llm"]["base_url"], "http://<redacted>@127.0.0.1:1/v1");
    assert_eq!(settings["transcribe"]["api"]["api_key"], "<set>");
    assert_eq!(settings["mcp_token"], "<set>");
    assert_eq!(settings["backends"]["transcriber"], "gemini");
    // Debug, which the logs use, leaves them out too.
    assert!(!format!("{:?}", w.settings).contains(MARKER));
}

#[tokio::test(flavor = "multi_thread")]
async fn status_says_the_llm_is_down_within_two_seconds() {
    let mut w = world();
    // Refused at once, and a black hole that never answers.
    for endpoint in ["127.0.0.1:1", "10.255.255.1:9"] {
        w.llm_endpoint = Some(endpoint.into());
        *w.app.lock().unwrap() = None;
        let started = std::time::Instant::now();
        let (st, status) = call(&w, "GET", "/api/v1/system/status", Some(&w.admin), None).await;
        let took = started.elapsed();
        assert_eq!(st, StatusCode::OK, "{status}");
        assert!(took < Duration::from_secs(2), "{endpoint} took {took:?}");
        assert_eq!(
            (status["llm"]["configured"].as_bool(), status["llm"]["reachable"].as_bool()),
            (Some(true), Some(false)),
            "{status}"
        );
        assert!(status["llm"]["error"].as_str().unwrap().contains(endpoint));
        assert_eq!(status["postgres"]["reachable"], true);
        assert_eq!(status["vaults"].as_array().unwrap().len(), 2);
        assert_eq!(status["caller_from_memory"], false);
        println!("llm at {endpoint}: status answered in {} ms", took.as_millis());
    }
    // An LLM that's off is said to be off, not down.
    w.llm_endpoint = None;
    *w.app.lock().unwrap() = None;
    let (_, status) = call(&w, "GET", "/api/v1/system/status", Some(&w.admin), None).await;
    assert_eq!(
        (status["llm"]["configured"].as_bool(), status["llm"]["reachable"].as_bool()),
        (Some(false), Some(false))
    );
    // One that listens is up.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    w.llm_endpoint = Some(listener.local_addr().unwrap().to_string());
    *w.app.lock().unwrap() = None;
    let (_, status) = call(&w, "GET", "/api/v1/system/status", Some(&w.admin), None).await;
    assert_eq!(status["llm"]["reachable"], true, "{status}");
}
