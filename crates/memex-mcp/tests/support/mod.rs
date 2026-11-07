//! A vault with one of everything, in memory, and real MCP clients to reach it with.
#![allow(dead_code)]
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
use memex_core::{NewToken, Server, ServerEdges, StepOutcome};
use memex_mcp::{Credential, DEFAULT_MAX_RESPONSE_BYTES, MCP_PATH, McpHttp, McpServer};
use memex_types::{Grant, PrincipalKind, Scope, VaultId};
use rmcp::model::{CallToolRequestParams, CallToolResult, JsonObject};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{ServiceError, ServiceExt};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

pub struct Fakes;

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
}

pub type S = Arc<Server<Fakes>>;
pub type Client = RunningService<RoleClient, ()>;

pub struct World {
    pub s: S,
    pub admin: String,
    pub a: VaultId,
    pub b: VaultId,
}

pub fn world() -> World {
    let s: S = Arc::new(Server::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_700_000_000_000),
        SeededRandom::new(7),
        memex_core::testing::mem_search(),
    ));
    let admin = s.bootstrap("root").unwrap().token;
    let c = s.authenticate(&admin).unwrap();
    let a = s.create_vault(&c, "vault-a").unwrap().id;
    let b = s.create_vault(&c, "vault-b").unwrap().id;
    World { s, admin, a, b }
}

impl World {
    pub fn token(&self, name: &str, grants: Vec<(VaultId, Scope)>) -> String {
        let c = self.s.authenticate(&self.admin).unwrap();
        let p = self.s.create_principal(&c, name, PrincipalKind::Agent, false).unwrap();
        let grants = grants.into_iter().map(|(vault, scope)| Grant { vault, scope }).collect();
        self.s
            .create_token(
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

    /// Runs the projectors and embedding jobs until nothing's left to do.
    pub fn settle(&self, vault: &VaultId) {
        for _ in 0..200 {
            self.s.engine().clock().advance(Duration::from_secs(60));
            while self.s.project_lexical(vault).unwrap() == StepOutcome::Progress {}
            while self.s.project_vector(vault).unwrap() == StepOutcome::Progress {}
            let Some(embed) = self.s.claim_job(vault, memex_core::EMBED_JOB, "w").unwrap() else {
                break;
            };
            self.s.run_embed_job(vault, &embed).unwrap();
            self.s.complete_job(vault, &embed, vec![]).unwrap();
        }
        self.s.engine().clock().advance(Duration::from_secs(60));
        while self.s.project_lexical(vault).unwrap() == StepOutcome::Progress {}
        while self.s.project_vector(vault).unwrap() == StepOutcome::Progress {}
    }

    /// Stores a text file and processes it, and returns its source and first segment.
    pub fn document(&self, vault: &VaultId, name: &str, tier: &str, text: &str) -> (String, String) {
        let c = self.s.authenticate(&self.admin).unwrap();
        let out = self
            .s
            .upload(
                &c,
                vault,
                memex_core::UploadRequest {
                    name: name.into(),
                    trust_tier: tier.into(),
                    metadata: json!({}),
                    replaces: None,
                },
                memex_core::UploadLimits::default(),
                &mut text.as_bytes(),
            )
            .unwrap();
        let source = out.source.header.id.clone();
        let job = self.s.claim_job(vault, memex_core::RENDITION_JOB, "w").unwrap().unwrap();
        let rows = self
            .s
            .build_rendition(
                vault,
                &source,
                memex_core::RenditionInput::Plain {
                    text: text.into(),
                    parser: "fixture-1".into(),
                },
            )
            .unwrap();
        assert!(self.s.complete_job(vault, &job, rows).unwrap());
        let rendition = self.s.list_renditions(&c, vault, &source).unwrap().remove(0);
        let segment = self.s.list_segments(&c, vault, &rendition.header.id, None, 10).unwrap().remove(0);
        (source.0, segment.header.id.0)
    }

    /// Proposes as admin and approves whatever waits for review, so it's all committed.
    pub fn commit(&self, vault: &VaultId, key: &str, ops: Value) {
        let c = self.s.authenticate(&self.admin).unwrap();
        let p = memex_core::Proposal {
            ops: serde_json::from_value(ops).unwrap(),
            rationale: "fixture".into(),
            idempotency_key: key.into(),
        };
        let out = self.s.propose(&c, vault, p, false).unwrap();
        let cs = out.changeset.expect("a changeset");
        if cs.state == memex_types::ChangeSetState::Pending {
            self.s.approve(&c, vault, &cs.header.id, None).unwrap();
        }
    }
}

pub fn claim_op(id: &str, segment: &str, quote: &str) -> Value {
    json!({"op": "add_assertion", "id": id, "subject": format!("s{id}"), "predicate": "says", "value": quote, "epistemic": "asserted", "machine": false, "confidence_permille": 900, "valid_from_ms": null, "valid_to_ms": null,
        "evidence": [[format!("{id}-e"), {"segment_id": segment, "quote": quote, "stance": "supports"}]]})
}

/// What the fixture holds, by id.
pub struct Fixture {
    pub source: String,
    pub segment: String,
    pub claim: String,
    pub page: String,
    pub other_page: String,
    pub entity: String,
    pub changeset: String,
    pub job: String,
}

pub fn fixture(w: &World) -> Fixture {
    let c = w.s.authenticate(&w.admin).unwrap();
    let text = "The pump station in Gothenburg opened in 1912 and still runs.";
    let (source, segment) = w.document(&w.a, "pump.txt", "untrusted", text);
    w.commit(&w.a, "claim", json!([claim_op("claim-1", &segment, "opened in 1912")]));
    let new_page = |title: &str, body: &str, key: &str| {
        let new = memex_core::NewPage {
            title: title.into(),
            slug: None,
            body: body.into(),
            rationale: "fixture".into(),
            idempotency_key: key.into(),
        };
        let (id, out) = w.s.propose_new_page(&c, &w.a, new, false).unwrap();
        let cs = out.changeset.unwrap();
        if cs.state == memex_types::ChangeSetState::Pending {
            w.s.approve(&c, &w.a, &cs.header.id, None).unwrap();
        }
        id.0
    };
    let other_page = new_page("Stations", "# Stations\n\nA list of pump stations.", "page-2");
    let page = new_page("Pump history", "# History\n\nThe pump is on [[Stations]].\n\nIt opened in 1912.", "page-1");
    let new = memex_core::NewEntity {
        type_name: "Person".into(),
        aliases: vec!["Ada Pump".into()],
        fields: vec![memex_core::FieldInput {
            field: "name".into(),
            value: json!("Ada Lovelace-Pump"),
            evidence: vec![],
            replaces: None,
        }],
        rationale: "fixture".into(),
        idempotency_key: "entity-1".into(),
    };
    let (entity, out) = w.s.propose_new_entity(&c, &w.a, new, false).unwrap();
    let cs = out.changeset.unwrap();
    let mut changeset = cs.header.id.0.clone();
    if cs.state == memex_types::ChangeSetState::Pending {
        changeset = w.s.approve(&c, &w.a, &cs.header.id, None).unwrap().header.id.0;
    }
    let job =
        w.s.list_jobs(&c, &w.a, Some(memex_core::RENDITION_JOB), None, None, 1)
            .unwrap()
            .remove(0)
            .header
            .id
            .0;
    w.settle(&w.a);
    Fixture {
        source,
        segment,
        claim: "claim-1".into(),
        page,
        other_page,
        entity: entity.0,
        changeset,
        job,
    }
}

/// A real client over an in-memory pipe, with the framing stdio uses. The binary's own stdin and stdout are covered by the inspector script in the spec's folder.
pub async fn connect_pipe(s: &S, token: &str, max: usize) -> Client {
    let (client_end, server_end) = tokio::io::duplex(1 << 24);
    let handler = McpServer::new(s.clone(), Credential::Token(Arc::new(token.to_string())), max);
    tokio::spawn(async move {
        if let Ok(running) = handler.serve(server_end).await {
            let _ = running.waiting().await;
        }
    });
    ().serve(client_end).await.expect("connects over a pipe")
}

/// The HTTP API and MCP on a real port, as `serve` runs them. Returns the base URL.
pub async fn start_http(s: &S, max: usize) -> String {
    let mcp = memex_mcp::routes(
        s.clone(),
        McpHttp {
            allowed_hosts: vec!["127.0.0.1".into()],
            max_response_bytes: max,
        },
    );
    let app = memex_http::router_with(s.clone(), memex_http::embedded_asset, mcp);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await;
    });
    format!("http://{addr}")
}

/// A real client over Streamable HTTP.
pub async fn connect_http(base: &str, token: &str) -> Client {
    let config = StreamableHttpClientTransportConfig::with_uri(format!("{base}{MCP_PATH}")).auth_header(token.to_string());
    ().serve(StreamableHttpClientTransport::from_config(config)).await.expect("connects over HTTP")
}

pub const CAP: usize = DEFAULT_MAX_RESPONSE_BYTES;

pub fn object(args: Value) -> JsonObject {
    args.as_object().cloned().unwrap_or_default()
}

pub async fn raw_call(c: &Client, tool: &str, args: Value) -> Result<CallToolResult, ServiceError> {
    c.peer()
        .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(object(args)))
        .await
}

pub fn text_of(r: &CallToolResult) -> String {
    r.content.iter().filter_map(|b| b.as_text().map(|t| t.text.clone())).collect()
}

/// Calls a tool that should work, and returns its envelope.
pub async fn ok(c: &Client, tool: &str, args: Value) -> Value {
    let r = raw_call(c, tool, args.clone()).await.expect("a tool result");
    let text = text_of(&r);
    assert_eq!(r.is_error, Some(false), "{tool} {args}: {text}");
    serde_json::from_str(&text).unwrap()
}

/// Calls a tool that should fail, and returns the API-format error it reports.
pub async fn failed(c: &Client, tool: &str, args: Value) -> Value {
    let r = raw_call(c, tool, args.clone()).await.expect("a tool result");
    let text = text_of(&r);
    assert_eq!(r.is_error, Some(true), "{tool} {args}: {text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    v["error"].clone()
}
