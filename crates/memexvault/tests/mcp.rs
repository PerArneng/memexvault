//! MCP on real Postgres and tantivy, over Streamable HTTP, beside the HTTP API. Run with `MEMEX_TEST_DATABASE_URL` set, as in AGENTS.md. Timings are taken on a machine shared with other work.
// These tests read the environment and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::testing::{FakeEmbedder, FakeRasterizer, FakeReranker, ManualClock, MemBlobStore, MemRegistry};
use memex_core::{Caller, NewPage, NewToken, Proposal, SearchEdges, Server, ServerEdges, StepOutcome, UploadLimits, UploadRequest};
use memex_lexical::TantivyLexical;
use memex_mcp::{DEFAULT_MAX_RESPONSE_BYTES, MCP_PATH, McpHttp};
use memex_store_postgres::PgStore;
use memex_types::{ChangeSetState, Grant, ObjectId, PrincipalKind, Scope, VaultId};
use memex_vector::PgVectors;
use memexvault::edges::{PgVectorIndex, SystemClock, SystemSeed, TantivyIndex};
use memexvault::pg_edge::PostgresStore;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
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
    admin: Caller,
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
    let pg = PgStore::connect(&url, 8).expect("connect");
    pg.migrate().expect("migrate");
    let pg = Arc::new(pg);
    let dir = std::env::temp_dir().join(format!("memex-mcp-{name}-{}", std::process::id()));
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
    let root = s.bootstrap("root").unwrap().token;
    let admin = s.authenticate(&root).unwrap();
    let v = s.create_vault(&admin, name).unwrap().id;
    Rig { s, admin, v, dir }
}

impl Rig {
    fn settle(&self) {
        for _ in 0..400 {
            let lexical = self.s.project_lexical(&self.v).unwrap();
            let vector = self.s.project_vector(&self.v).unwrap();
            let embed = self.s.claim_job(&self.v, memex_core::EMBED_JOB, "w").unwrap();
            if let Some(job) = &embed {
                self.s.run_embed_job(&self.v, job).unwrap();
                self.s.complete_job(&self.v, job, vec![]).unwrap();
            }
            if lexical == StepOutcome::Idle && vector == StepOutcome::Idle && embed.is_none() {
                std::thread::sleep(Duration::from_millis(600));
                if self.s.project_lexical(&self.v).unwrap() == StepOutcome::Idle {
                    return;
                }
            }
        }
        panic!("the projectors never settled");
    }

    fn token(&self, name: &str, scope: Scope) -> String {
        let p = self.s.create_principal(&self.admin, name, PrincipalKind::Agent, false).unwrap();
        let new = NewToken {
            principal_id: Some(p.id),
            name: "t".into(),
            grants: vec![Grant { vault: self.v.clone(), scope }],
            expires_at_ms: None,
        };
        self.s.create_token(&self.admin, new).unwrap().token
    }
}

struct Ids {
    source: String,
    segment: String,
    claim: String,
    page: String,
    job: String,
}

fn fixture(r: &Rig) -> Ids {
    let text = "The pump station in Gothenburg opened in 1912 and still runs.";
    let request = UploadRequest {
        name: "pump.txt".into(),
        trust_tier: "untrusted".into(),
        metadata: json!({}),
        replaces: None,
    };
    let up = r.s.upload(&r.admin, &r.v, request, UploadLimits::default(), &mut text.as_bytes()).unwrap();
    let source = up.source.header.id.clone();
    let job = r.s.claim_job(&r.v, memex_core::RENDITION_JOB, "w").unwrap().unwrap();
    let rows =
        r.s.build_rendition(
            &r.v,
            &source,
            memex_core::RenditionInput::Plain {
                text: text.into(),
                parser: "fixture-1".into(),
            },
        )
        .unwrap();
    assert!(r.s.complete_job(&r.v, &job, rows).unwrap());
    let rendition = r.s.list_renditions(&r.admin, &r.v, &source).unwrap().remove(0);
    let segment = r.s.list_segments(&r.admin, &r.v, &rendition.header.id, None, 1).unwrap().remove(0).header.id.0;
    let op = json!({"op": "add_assertion", "id": "claim-1", "subject": "pump", "predicate": "opened", "value": "1912", "epistemic": "asserted", "machine": false, "confidence_permille": 900, "valid_from_ms": null, "valid_to_ms": null,
        "evidence": [["claim-1-e", {"segment_id": segment, "quote": "opened in 1912", "stance": "supports"}]]});
    let p = Proposal {
        ops: serde_json::from_value(json!([op])).unwrap(),
        rationale: "fixture".into(),
        idempotency_key: "claim".into(),
    };
    let out = r.s.propose(&r.admin, &r.v, p, false).unwrap();
    let cs = out.changeset.unwrap();
    if cs.state == ChangeSetState::Pending {
        r.s.approve(&r.admin, &r.v, &cs.header.id, None).unwrap();
    }
    let page = |title: &str, body: &str| -> ObjectId {
        let new = NewPage {
            title: title.into(),
            slug: None,
            body: body.into(),
            rationale: "fixture".into(),
            idempotency_key: title.into(),
        };
        let (id, out) = r.s.propose_new_page(&r.admin, &r.v, new, false).unwrap();
        let cs = out.changeset.unwrap();
        if cs.state == ChangeSetState::Pending {
            r.s.approve(&r.admin, &r.v, &cs.header.id, None).unwrap();
        }
        id
    };
    page("Stations", "A list of pump stations.");
    let linking = page("Pump history", "The pump is on [[Stations]].\n\nA link to [[Nowhere]].");
    r.settle();
    Ids {
        source: source.0,
        segment,
        claim: "claim-1".into(),
        page: linking.0,
        job: job.header.id.0,
    }
}

async fn call(client: &rmcp::service::RunningService<rmcp::service::RoleClient, ()>, tool: &str, args: Value) -> (bool, Value) {
    let params = CallToolRequestParams::new(tool.to_string()).with_arguments(args.as_object().cloned().unwrap());
    let r = client.peer().call_tool(params).await.expect("a tool result");
    let text: String = r.content.iter().filter_map(|b| b.as_text().map(|t| t.text.clone())).collect();
    (r.is_error == Some(true), serde_json::from_str(&text).unwrap())
}

#[test]
#[ignore = "needs Postgres"]
fn pg_mcp_every_tool_matches_http_on_real_storage() {
    let r = rig("mcp-tools");
    let f = fixture(&r);
    let (reader, proposer) = (r.token("reader", Scope::Read), r.token("proposer", Scope::Propose));
    let v = r.v.0.clone();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let mcp = memex_mcp::routes(
            r.s.clone(),
            McpHttp {
                allowed_hosts: vec!["127.0.0.1".into()],
                max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            },
        );
        let app = memex_http::router_with(r.s.clone(), memex_http::embedded_asset, mcp);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum_serve(listener, app).await;
        });
        let base = format!("http://{addr}");
        let connect = |token: String| {
            let config = StreamableHttpClientTransportConfig::with_uri(format!("{base}{MCP_PATH}")).auth_header(token);
            async move { ().serve(StreamableHttpClientTransport::from_config(config)).await.expect("connects") }
        };
        let client = connect(proposer.clone()).await;
        let path = format!("/api/v1/vaults/{v}");
        let http = reqwest::Client::new();
        let get = |p: String| {
            let (http, token, base) = (http.clone(), proposer.clone(), base.clone());
            async move {
                http.get(format!("{base}{p}"))
                    .bearer_auth(token)
                    .send()
                    .await
                    .unwrap()
                    .json::<Value>()
                    .await
                    .unwrap()
            }
        };
        let cases = vec![
            ("get", json!({"kind": "page", "id": f.page}), format!("{path}/pages/{}", f.page)),
            ("get", json!({"kind": "source", "id": f.source}), format!("{path}/sources/{}", f.source)),
            ("get", json!({"kind": "segment", "id": f.segment}), format!("{path}/segments/{}", f.segment)),
            ("get", json!({"kind": "job", "id": f.job}), format!("{path}/jobs/{}", f.job)),
            ("neighborhood", json!({"id": f.page}), format!("{path}/objects/{}/neighborhood", f.page)),
            ("resolve", json!({"mention": "nobody"}), format!("{path}/entities/resolve?mention=nobody")),
            ("changes", json!({"limit": 5}), format!("{path}/changes?limit=5")),
            ("provenance", json!({"id": f.claim}), format!("{path}/assertions/{}/lineage", f.claim)),
            (
                "search",
                json!({"q": "Gothenburg", "mode": "lexical"}),
                format!("{path}/search?q=Gothenburg&mode=lexical"),
            ),
        ];
        for (tool, mut args, route) in cases {
            args["vault"] = json!(v);
            args["detail"] = json!("detailed");
            let started = Instant::now();
            let (failed, envelope) = call(&client, tool, args.clone()).await;
            let took = started.elapsed();
            assert!(!failed, "{tool}: {envelope}");
            assert_eq!(envelope["data"], get(route.clone()).await, "{tool} against {route}");
            println!("{tool}: {:.1} ms over MCP on real storage (busy machine)", took.as_secs_f64() * 1000.0);
        }
        let (_, links) = call(&client, "neighborhood", json!({"vault": v, "id": f.page})).await;
        assert_eq!(links["data"]["items"].as_array().unwrap().len(), 2, "{links}");

        // Writes go through to the log and are readable after.
        let (failed, up) = call(&client, "ingest", json!({"vault": v, "name": "note.txt", "text": "a note about the pump"})).await;
        assert!(!failed, "{up}");
        assert!(up["data"]["job_id"].is_string(), "{up}");
        let op = json!({"op": "add_edge", "id": "edge-1", "from": f.claim, "to": f.page, "kind": "about"});
        let (failed, proposed) = call(
            &client,
            "propose",
            json!({"vault": v, "ops": [op], "rationale": "a link", "idempotency_key": "edge-1"}),
        )
        .await;
        assert!(!failed, "{proposed}");

        // The scopes are the store's vault scopes, on Postgres too.
        let read_client = connect(reader.clone()).await;
        let (failed, denied) = call(
            &read_client,
            "propose",
            json!({"vault": v, "ops": [op], "rationale": "x", "idempotency_key": "denied"}),
        )
        .await;
        assert!(failed);
        assert_eq!(denied["error"]["code"], "forbidden");
        let (failed, denied) = call(&read_client, "ingest", json!({"vault": v, "name": "n.txt", "text": "denied"})).await;
        assert!(failed);
        assert_eq!(denied["error"]["code"], "forbidden");
        let (failed, _) = call(&read_client, "get", json!({"vault": v, "kind": "page", "id": f.page})).await;
        assert!(!failed);
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    drop(r);
}

/// So this file names axum's serve once, and the service type stays out of the test.
async fn axum_serve(listener: tokio::net::TcpListener, app: axum::Router) -> std::io::Result<()> {
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await
}
