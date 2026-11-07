//! The audit agent's routes end to end, in memory, with a scripted agent where one is needed at all. Every number here says something about the routes and nothing about a model.
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
use memex_core::{AUDIT_JOB, Caller, Clock, NewToken, RENDITION_JOB, RenditionInput, Server, ServerEdges, Snapshot, Store, UploadLimits, UploadRequest};
use memex_http::{openapi, router};
use memex_types::{Epistemic, EvidenceInput, Grant, ObjectId, Op, PrincipalKind, Row, Scope, Stance, Table, VaultId};
use serde_json::{Value, json};
use std::sync::Arc;
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

struct World {
    s: Arc<Server<Fakes>>,
    root: String,
    admin: Caller,
    reader: String,
    agent: String,
    v: VaultId,
}

fn world() -> World {
    let s = Arc::new(Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_700_000_000_000),
        SeededRandom::new(26),
        memex_core::testing::mem_search(),
    ));
    let root = s.bootstrap("root").unwrap().token;
    let admin = s.authenticate(&root).unwrap();
    let v = s.create_vault(&admin, "v").unwrap().id;
    let token = |name: &str, kind, scope| {
        let p = s.create_principal(&admin, name, kind, false).unwrap();
        s.create_token(
            &admin,
            NewToken {
                principal_id: Some(p.id),
                name: "t".into(),
                grants: vec![Grant { vault: v.clone(), scope }],
                expires_at_ms: None,
            },
        )
        .unwrap()
        .token
    };
    let reader = token("reader", PrincipalKind::Human, Scope::Read);
    let agent = token("agent", PrincipalKind::Agent, Scope::Propose);
    World { s, root, admin, reader, agent, v }
}

async fn call(w: &World, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"));
    if body.is_some() {
        req = req.header("content-type", "application/json");
    }
    let req = req.body(body.map_or(Body::empty(), |b| Body::from(b.to_string()))).unwrap();
    let res = router(w.s.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 10_000_000).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

impl World {
    fn base(&self) -> String {
        format!("/api/v1/vaults/{}/audit", self.v)
    }

    /// Claims a person committed, each with its own quote in one document.
    fn claims(&self, count: usize) {
        let sentences: Vec<String> = (0..count).map(|n| format!("Fartyget nummer {n} byggdes i Stockholm.")).collect();
        let text = sentences.join(" ");
        let req = UploadRequest {
            name: "d.txt".into(),
            trust_tier: "trusted".into(),
            metadata: json!({}),
            replaces: None,
        };
        let source = self.s.upload(&self.admin, &self.v, req, UploadLimits::default(), &mut text.as_bytes()).unwrap().source.header.id;
        let job = self.s.claim_job(&self.v, RENDITION_JOB, "p").unwrap().unwrap();
        let rows = self
            .s
            .build_rendition(
                &self.v,
                &source,
                RenditionInput::Plain {
                    text: text.clone(),
                    parser: "plain-1".into(),
                },
            )
            .unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
        let segment = self
            .s
            .engine()
            .store()
            .snapshot(&self.v)
            .unwrap()
            .rows(Table::Segment, None)
            .unwrap()
            .into_iter()
            .find_map(|r| if let Row::Segment(s) = r { Some(s.header.id) } else { None })
            .unwrap();
        for (n, quote) in sentences.iter().enumerate() {
            let p = memex_core::Proposal {
                ops: vec![Op::AddAssertion {
                    id: ObjectId(format!("claim-{n:03}")),
                    subject: format!("Fartyget {n}"),
                    predicate: "byggdes i".into(),
                    value: json!("Stockholm"),
                    epistemic: Epistemic::Asserted,
                    machine: false,
                    confidence_permille: 1000,
                    valid_from_ms: None,
                    valid_to_ms: None,
                    evidence: vec![(
                        ObjectId(format!("evidence-{n:03}")),
                        EvidenceInput {
                            segment_id: segment.clone(),
                            quote: quote.clone(),
                            stance: Stance::Supports,
                        },
                    )],
                }],
                rationale: "read it".into(),
                idempotency_key: format!("c{n}"),
            };
            self.s.propose(&self.admin, &self.v, p, false).unwrap();
        }
    }

    fn run_next_job(&self) {
        let job = self.s.claim_job(&self.v, AUDIT_JOB, "w").unwrap().expect("an audit job");
        let rows = self.s.run_audit_job(&self.v, &job, None::<&memex_core::testing::ScriptedAgent>).unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
    }

    async fn turn_on(&self, interval_ms: i64) {
        let (status, body) = call(
            self,
            "PUT",
            &format!("{}/schedule", self.base()),
            &self.root,
            Some(json!({"enabled": true, "interval_ms": interval_ms, "sample_size": 10, "run_token_budget": 1000, "day_token_budget": 5000, "ttl_days": {"byggdes i": 30}})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
}

#[tokio::test]
async fn a_run_through_the_route_samples_the_same_as_a_scheduled_run_with_that_seed() {
    let w = world();
    w.claims(40);
    w.turn_on(60_000).await;
    w.s.engine().clock().advance(Duration::from_millis(60_000));
    let scheduled = w.s.audit_tick(&w.v).unwrap().expect("due");
    w.run_next_job();
    let (status, detail) = call(&w, "GET", &format!("{}/runs/{}", w.base(), scheduled.header.id), &w.reader, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["run"]["trigger"], "scheduled");
    assert_eq!(detail["run"]["state"], "finished");
    let seed = detail["run"]["seed"].as_str().unwrap().to_string();
    assert_eq!(seed, scheduled.seed.to_string());
    let sample = detail["sample"].as_array().unwrap().clone();
    assert_eq!(sample.len(), 10);

    let (status, started) = call(&w, "POST", &format!("{}/runs", w.base()), &w.root, Some(json!({"seed": seed, "sample_size": 10}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    assert_eq!(started["trigger"], "on_demand");
    assert_eq!(started["state"], "queued");
    w.run_next_job();
    let (_, by_route) = call(&w, "GET", &format!("{}/runs/{}", w.base(), started["id"].as_str().unwrap()), &w.reader, None).await;
    assert_eq!(by_route["sample"].as_array().unwrap(), &sample, "the same claims, in the same order");
    assert_eq!(by_route["run"]["seed"], json!(seed));

    let (_, other) = call(&w, "POST", &format!("{}/runs", w.base()), &w.root, Some(json!({"seed": "1", "sample_size": 10}))).await;
    w.run_next_job();
    let (_, other) = call(&w, "GET", &format!("{}/runs/{}", w.base(), other["id"].as_str().unwrap()), &w.reader, None).await;
    assert_ne!(other["sample"].as_array().unwrap(), &sample, "and another seed differs");

    let (_, list) = call(&w, "GET", &format!("{}/runs?limit=2", w.base()), &w.reader, None).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 2);
    assert!(list["next_cursor"].is_string());
    let (_, rest) = call(&w, "GET", &format!("{}/runs?limit=2&cursor={}", w.base(), list["next_cursor"].as_str().unwrap()), &w.reader, None).await;
    assert_eq!(rest["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn findings_come_with_each_check_s_result_and_their_review_item() {
    let w = world();
    w.claims(3);
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    let mut e = snap.evidence_of(&[ObjectId("claim-001".into())]).unwrap().remove(0);
    drop(snap);
    e.quote = "något annat".into();
    w.s.apply_as(&w.admin, &w.v, "state.recorded", 1, json!({"mutations": [{"put": {"evidence": e}}]})).unwrap();
    let (_, started) = call(&w, "POST", &format!("{}/runs", w.base()), &w.root, Some(json!({"seed": "5"}))).await;
    w.run_next_job();
    let run = started["id"].as_str().unwrap();
    let (_, all) = call(&w, "GET", &format!("{}/runs/{run}/checks", w.base()), &w.reader, None).await;
    assert_eq!(all["items"].as_array().unwrap().len(), 3);
    let (_, found) = call(&w, "GET", &format!("{}/runs/{run}/checks?findings=true", w.base()), &w.reader, None).await;
    let found = found["items"].as_array().unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["claim_id"], "claim-001");
    assert_eq!(found[0]["outcome"], "failed");
    assert_eq!(found[0]["checks"][0]["check"], "quote_relocation");
    assert_eq!(found[0]["checks"][0]["status"], "fail");
    assert_eq!(found[0]["checks"][0]["cause"], "quote_missing");
    let item = found[0]["changeset_id"].as_str().unwrap();
    let (status, review) = call(&w, "GET", &format!("/api/v1/vaults/{}/review/items/{item}", w.v), &w.reader, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(review["needs_a_person"], true);
    assert_eq!(review["changeset"]["state"], "pending");
    assert_eq!(review["evidence"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn an_agent_with_propose_scope_is_refused_every_direct_write_and_a_read_token_every_write() {
    let w = world();
    w.claims(2);
    let (_, started) = call(&w, "POST", &format!("{}/runs", w.base()), &w.root, Some(json!({"seed": "5"}))).await;
    w.run_next_job();
    assert!(started["id"].is_string());
    // Its own retraction goes to the queue.
    let (status, proposed) = call(
        &w,
        "POST",
        &format!("/api/v1/vaults/{}/proposals", w.v),
        &w.agent,
        Some(json!({"ops": [{"op": "retract_assertion", "target": "claim-000", "base_version": 1}], "rationale": "looks wrong", "idempotency_key": "a"})),
    )
    .await;
    assert!(status.is_success(), "{status} {proposed}");
    assert_eq!(proposed["changeset"]["state"], "pending");
    let cs = proposed["changeset"]["id"].as_str().unwrap();
    for (token, name) in [(&w.agent, "agent"), (&w.reader, "reader")] {
        for (method, path, body) in [
            ("POST", format!("/api/v1/vaults/{}/changesets/{cs}/approve", w.v), Some(json!({}))),
            ("POST", format!("/api/v1/vaults/{}/changesets/{cs}/reject", w.v), Some(json!({"reason": "no"}))),
            ("POST", format!("/api/v1/vaults/{}/review/batch", w.v), Some(json!({"decisions": []}))),
            ("POST", format!("{}/runs", w.base()), Some(json!({}))),
            ("POST", format!("{}/test-vault", w.base()), None),
            ("POST", format!("{}/planted", w.base()), Some(json!({"seed": "1", "per_kind": 1}))),
            (
                "PUT",
                format!("{}/schedule", w.base()),
                Some(json!({"enabled": true, "interval_ms": 60000, "sample_size": 1, "run_token_budget": 0, "day_token_budget": 0})),
            ),
        ] {
            let (status, body) = call(&w, method, &path, token, body).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{name} {method} {path}: {body}");
        }
    }
    let row = w.s.get_changeset(&w.admin, &w.v, &ObjectId(cs.into())).unwrap();
    assert_eq!(row.decided_by, None, "denials change nothing");
    // A person can, and then the claim leaves search and its row says so.
    let (status, approved) = call(&w, "POST", &format!("/api/v1/vaults/{}/changesets/{cs}/approve", w.v), &w.root, Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["state"], "committed");
}

#[tokio::test]
async fn reads_need_read_scope_and_an_unknown_run_or_another_vault_is_not_found() {
    let w = world();
    let other = w.s.create_vault(&w.admin, "other").unwrap().id;
    for path in ["runs", "schedule", "budget", "recall"] {
        let (status, body) = call(&w, "GET", &format!("{}/{path}", w.base()), &w.reader, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        let (status, _) = call(&w, "GET", &format!("/api/v1/vaults/{other}/audit/{path}"), &w.reader, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let (status, _) = call(&w, "GET", &format!("{}/runs/nope", w.base()), &w.reader, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&w, "GET", &format!("{}/runs/nope/checks", w.base()), &w.reader, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&w, "GET", &format!("{}/runs", w.base()), "mvx_wrong", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn schedule_and_budget_read_back_and_a_bad_request_is_refused() {
    let w = world();
    let (_, off) = call(&w, "GET", &format!("{}/schedule", w.base()), &w.reader, None).await;
    assert_eq!(off["enabled"], false, "off until someone turns it on");
    assert!(off["next_run_at_ms"].is_null());
    w.turn_on(60_000).await;
    let (_, on) = call(&w, "GET", &format!("{}/schedule", w.base()), &w.reader, None).await;
    assert_eq!(on["enabled"], true);
    assert_eq!(on["next_run_at_ms"], json!(w.s.engine().clock().now_ms() + 60_000));
    assert_eq!(on["ttl_days"]["byggdes i"], 30);
    let (_, budget) = call(&w, "GET", &format!("{}/budget", w.base()), &w.reader, None).await;
    assert_eq!((budget["run_token_budget"].as_u64(), budget["day_token_budget"].as_u64()), (Some(1000), Some(5000)));
    assert_eq!((budget["day_spent"].as_u64(), budget["day_remaining"].as_u64()), (Some(0), Some(5000)));
    for bad in [
        json!({"enabled": true, "interval_ms": 1, "sample_size": 1, "run_token_budget": 0, "day_token_budget": 0}),
        json!({"enabled": true, "interval_ms": 60000, "sample_size": 0, "run_token_budget": 0, "day_token_budget": 0}),
        json!({"enabled": true, "interval_ms": 60000, "sample_size": 1, "run_token_budget": 0, "day_token_budget": 0, "ttl_days": {"p": 0}}),
    ] {
        let (status, body) = call(&w, "PUT", &format!("{}/schedule", w.base()), &w.root, Some(bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body["error"]["message"].is_string() || body["message"].is_string(), "{body}");
    }
    for bad in [json!({"seed": "not a number"}), json!({"sample_size": 0}), json!({"sample_size": 100000})] {
        let (status, body) = call(&w, "POST", &format!("{}/runs", w.base()), &w.root, Some(bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
}

#[tokio::test]
async fn planted_errors_and_their_report_over_http() {
    let w = world();
    let (status, _) = call(&w, "POST", &format!("{}/planted", w.base()), &w.root, Some(json!({"seed": "1", "per_kind": 1}))).await;
    assert_eq!(status, StatusCode::CONFLICT, "only in a test vault");
    let (status, body) = call(&w, "POST", &format!("{}/test-vault", w.base()), &w.root, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    w.claims(6);
    let (status, _) = call(&w, "POST", &format!("{}/test-vault", w.base()), &w.root, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "it isn't empty any more");
    let (status, body) = call(&w, "POST", &format!("{}/planted", w.base()), &w.root, Some(json!({"seed": "1", "per_kind": 1, "kinds": ["nonsense"]}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = call(
        &w,
        "POST",
        &format!("{}/planted", w.base()),
        &w.root,
        Some(json!({"seed": "1", "per_kind": 2, "kinds": ["altered_quote", "wrong_value"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["kinds"].as_array().unwrap().len(), 2);
    call(&w, "POST", &format!("{}/runs", w.base()), &w.root, Some(json!({"seed": "1"}))).await;
    w.run_next_job();
    let (_, report) = call(&w, "GET", &format!("{}/recall", w.base()), &w.reader, None).await;
    assert_eq!(report["deterministic"]["planted"], 2);
    assert_eq!(report["deterministic"]["caught"], 2);
    assert_eq!(report["llm_only"]["planted"], 2);
    assert_eq!(report["llm_only"]["caught"], 0);
    assert_eq!(report["llm_evidence"], "none");
    assert_eq!(report["target_met"], Value::Null);
    assert_eq!(report["too_small"], true);
    assert_eq!(report["clean_audited"], 2);
    assert_eq!(report["kinds"].as_array().unwrap().len(), 2);
}

#[test]
fn the_openapi_document_has_the_audit_routes() {
    let doc = serde_json::to_value(openapi()).unwrap();
    for path in ["runs", "runs/{run}", "runs/{run}/checks", "schedule", "budget", "recall", "test-vault", "planted"] {
        assert!(doc["paths"].get(format!("/api/v1/vaults/{{id}}/audit/{path}")).is_some(), "{path}");
    }
}
