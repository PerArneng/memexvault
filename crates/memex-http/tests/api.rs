//! The HTTP API end to end, in memory: real routing and JSON, with fake edges.
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
use memex_core::{Clock, NewToken, Registry, Server, ServerEdges, Store};
use memex_http::{BODY_LIMIT, openapi, router};
use memex_types::{Grant, PrincipalKind, Scope, VaultId};
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
}

fn server() -> Arc<Server<Fakes>> {
    Arc::new(Server::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_700_000_000_000),
        SeededRandom::new(7),
        memex_core::testing::mem_search(),
    ))
}

async fn call(s: &Arc<Server<Fakes>>, method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value, axum::http::HeaderMap) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = router(s.clone()).oneshot(req).await.unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = to_bytes(res.into_body(), 4 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), headers)
}

struct World {
    s: Arc<Server<Fakes>>,
    admin: String,
    a: VaultId,
    b: VaultId,
}

fn world() -> World {
    let s = server();
    let admin = s.bootstrap("root").unwrap().token;
    let c = s.authenticate(&admin).unwrap();
    let a = s.create_vault(&c, "vault-a").unwrap().id;
    let b = s.create_vault(&c, "vault-b").unwrap().id;
    World { s, admin, a, b }
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

#[tokio::test]
async fn the_scope_matrix_has_no_gaps_and_denials_change_nothing() {
    use StatusCode as S;
    // role: scope in vault A (server admin handled separately). Columns are read, propose (none yet), admin, none.
    struct Case {
        method: &'static str,
        path: String,
        body: Option<Value>,
        /// Expected for Read, Propose, Admin scope in vault A, and for a token with no grants.
        expected: [S; 4],
        changes_state: bool,
    }
    let w = world();
    let (ra, rb) = (w.a.0.clone(), w.b.0.clone());
    let grant_a = json!({"name": "new", "grants": [{"vault_id": ra, "scope": "read"}]});
    let grant_b = json!({"name": "new", "grants": [{"vault_id": rb, "scope": "read"}]});
    let cases = vec![
        Case {
            method: "GET",
            path: "/api/v1/whoami".into(),
            body: None,
            expected: [S::OK; 4],
            changes_state: false,
        },
        Case {
            method: "GET",
            path: "/api/v1/vaults".into(),
            body: None,
            expected: [S::OK; 4],
            changes_state: false,
        },
        Case {
            method: "POST",
            path: "/api/v1/vaults".into(),
            body: Some(json!({"name": "x"})),
            expected: [S::FORBIDDEN; 4],
            changes_state: true,
        },
        Case {
            method: "GET",
            path: format!("/api/v1/vaults/{ra}"),
            body: None,
            expected: [S::OK, S::OK, S::OK, S::NOT_FOUND],
            changes_state: false,
        },
        Case {
            method: "GET",
            path: format!("/api/v1/vaults/{rb}"),
            body: None,
            expected: [S::NOT_FOUND; 4],
            changes_state: false,
        },
        Case {
            method: "PATCH",
            path: format!("/api/v1/vaults/{ra}"),
            body: Some(json!({"name": "vault-a"})),
            expected: [S::FORBIDDEN, S::FORBIDDEN, S::OK, S::NOT_FOUND],
            changes_state: true,
        },
        Case {
            method: "PATCH",
            path: format!("/api/v1/vaults/{rb}"),
            body: Some(json!({"name": "y"})),
            expected: [S::NOT_FOUND; 4],
            changes_state: true,
        },
        Case {
            method: "DELETE",
            path: format!("/api/v1/vaults/{rb}"),
            body: None,
            expected: [S::NOT_FOUND; 4],
            changes_state: true,
        },
        Case {
            method: "GET",
            path: "/api/v1/principals".into(),
            body: None,
            expected: [S::FORBIDDEN; 4],
            changes_state: false,
        },
        Case {
            method: "POST",
            path: "/api/v1/principals".into(),
            body: Some(json!({"name": "z", "kind": "agent"})),
            expected: [S::FORBIDDEN; 4],
            changes_state: true,
        },
        Case {
            method: "POST",
            path: "/api/v1/principals/whoever/disable".into(),
            body: None,
            expected: [S::FORBIDDEN; 4],
            changes_state: true,
        },
        Case {
            method: "GET",
            path: "/api/v1/tokens".into(),
            body: None,
            expected: [S::OK; 4],
            changes_state: false,
        },
        Case {
            method: "GET",
            path: "/api/v1/tokens?all=true".into(),
            body: None,
            expected: [S::FORBIDDEN; 4],
            changes_state: false,
        },
        Case {
            method: "POST",
            path: "/api/v1/tokens".into(),
            body: Some(grant_a),
            expected: [S::FORBIDDEN, S::FORBIDDEN, S::CREATED, S::NOT_FOUND],
            changes_state: true,
        },
        Case {
            method: "POST",
            path: "/api/v1/tokens".into(),
            body: Some(grant_b),
            expected: [S::NOT_FOUND; 4],
            changes_state: true,
        },
        Case {
            method: "DELETE",
            path: "/api/v1/tokens/not-a-token".into(),
            body: None,
            expected: [S::NOT_FOUND; 4],
            changes_state: true,
        },
    ];
    let roles = [Some(Scope::Read), Some(Scope::Propose), Some(Scope::Admin), None];
    let tokens: Vec<String> = roles.iter().enumerate().map(|(i, r)| token_with(&w, &format!("role{i}"), *r)).collect();
    for case in &cases {
        for (i, token) in tokens.iter().enumerate() {
            let before = w.s.registry().fingerprint();
            let (status, body, _) = call(&w.s, case.method, &case.path, Some(token), case.body.clone()).await;
            assert_eq!(status, case.expected[i], "{} {} role {i}: {body}", case.method, case.path);
            if !status.is_success() {
                assert_eq!(w.s.registry().fingerprint(), before, "{} {} role {i} changed state", case.method, case.path);
                assert!(body["error"]["request_id"].is_string(), "{body}");
            } else if !case.changes_state {
                assert_eq!(w.s.registry().fingerprint(), before, "{} {} role {i} changed state", case.method, case.path);
            }
        }
    }
}

#[tokio::test]
async fn a_server_admin_passes_the_same_routes() {
    let w = world();
    let ra = w.a.0.clone();
    let t = Some(w.admin.as_str());
    for (m, p, b, want) in [
        ("GET", format!("/api/v1/vaults/{ra}"), None, StatusCode::OK),
        ("PATCH", format!("/api/v1/vaults/{ra}"), Some(json!({"name": "renamed"})), StatusCode::OK),
        ("POST", "/api/v1/vaults".to_string(), Some(json!({"name": "third"})), StatusCode::CREATED),
        ("GET", "/api/v1/principals".to_string(), None, StatusCode::OK),
        (
            "POST",
            "/api/v1/principals".to_string(),
            Some(json!({"name": "agent", "kind": "agent"})),
            StatusCode::CREATED,
        ),
        ("GET", "/api/v1/tokens?all=true".to_string(), None, StatusCode::OK),
        ("DELETE", format!("/api/v1/vaults/{ra}"), None, StatusCode::NO_CONTENT),
    ] {
        let (status, body, _) = call(&w.s, m, &p, t, b).await;
        assert_eq!(status, want, "{m} {p}: {body}");
    }
    assert_eq!(call(&w.s, "GET", &format!("/api/v1/vaults/{ra}"), t, None).await.0, StatusCode::NOT_FOUND);
    assert!(w.s.engine().store().snapshot(&w.a).is_err());
}

#[tokio::test]
async fn a_token_for_a_can_not_tell_that_b_exists() {
    let w = world();
    let t = token_with(&w, "a-reader", Some(Scope::Read));
    let strip = |mut v: Value| {
        v["error"]["request_id"] = Value::Null;
        v
    };
    for (m, body) in [("GET", None), ("PATCH", Some(json!({"name": "q"}))), ("DELETE", None)] {
        let real = call(&w.s, m, &format!("/api/v1/vaults/{}", w.b.0), Some(&t), body.clone()).await;
        let fake = call(&w.s, m, "/api/v1/vaults/does-not-exist", Some(&t), body).await;
        assert_eq!(real.0, fake.0, "{m}");
        assert_eq!(strip(real.1), strip(fake.1), "{m}");
    }
    let (_, listed, _) = call(&w.s, "GET", "/api/v1/vaults", Some(&t), None).await;
    let ids: Vec<&str> = listed["items"].as_array().unwrap().iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![w.a.0.as_str()]);
}

#[tokio::test]
async fn every_write_records_who_made_it() {
    let w = world();
    let c = w.s.authenticate(&w.admin).unwrap();
    let writer = token_with(&w, "writer", Some(Scope::Propose));
    let wc = w.s.authenticate(&writer).unwrap();
    w.s.apply_as(
        &wc,
        &w.a,
        "page.created",
        1,
        json!({"id": "p1", "title": "T", "body": "B", "actor": "someone else"}),
    )
    .unwrap();
    let events = w.s.read_events(&c, &w.a, None).unwrap();
    assert_eq!(events.len(), 1);
    // The body can't choose the actor: it comes from the token.
    assert_eq!(events[0].actor, wc.principal.id);
    let reader = token_with(&w, "reader", Some(Scope::Read));
    let rc = w.s.authenticate(&reader).unwrap();
    assert!(
        w.s.apply_as(&rc, &w.a, "page.created", 1, json!({"id": "p2", "title": "T", "body": "B"}))
            .is_err()
    );
    assert_eq!(w.s.read_events(&c, &w.a, None).unwrap().len(), 1);
    // Registry writes carry an actor too.
    for entry in w.s.registry().audit_log(1000).unwrap() {
        assert!(!entry.actor.is_empty());
        assert!(entry.time_ms > 0);
    }
    let log = w.s.registry().audit_log(1000).unwrap();
    assert!(log.iter().any(|a| a.action == "vault.create" && a.actor == c.principal.id));
    assert!(log.iter().any(|a| a.action == "principal.create" && a.actor == "bootstrap"));
}

#[tokio::test]
async fn bad_tokens_all_look_the_same() {
    let w = world();
    let c = w.s.authenticate(&w.admin).unwrap();
    let p = w.s.create_principal(&c, "tmp", PrincipalKind::Human, false).unwrap();
    let new = |e| NewToken {
        principal_id: Some(p.id.clone()),
        name: "t".into(),
        grants: vec![],
        expires_at_ms: e,
    };
    let now = w.s.engine().clock().now_ms();
    let revoked = w.s.create_token(&c, new(None)).unwrap();
    w.s.revoke_token(&c, &revoked.row.id).unwrap();
    let expiring = w.s.create_token(&c, new(Some(now + 1000))).unwrap();
    let disabled_owner = {
        let q = w.s.create_principal(&c, "gone", PrincipalKind::Human, false).unwrap();
        let t =
            w.s.create_token(
                &c,
                NewToken {
                    principal_id: Some(q.id.clone()),
                    name: "t".into(),
                    grants: vec![],
                    expires_at_ms: None,
                },
            )
            .unwrap();
        w.s.disable_principal(&c, &q.id).unwrap();
        t
    };
    assert_eq!(call(&w.s, "GET", "/api/v1/whoami", Some(&expiring.token), None).await.0, StatusCode::OK);
    w.s.engine().clock().advance(Duration::from_millis(1000));
    let long = format!("mvx_{}", "a".repeat(5000));
    let mut bodies = vec![];
    for t in [
        Some("mvx_wrong"),
        Some("garbage"),
        Some(long.as_str()),
        Some(revoked.token.as_str()),
        Some(expiring.token.as_str()),
        Some(disabled_owner.token.as_str()),
        None,
    ] {
        let (status, mut body, _) = call(&w.s, "GET", "/api/v1/whoami", t, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{t:?}");
        body["error"]["request_id"] = Value::Null;
        bodies.push(body);
    }
    assert!(bodies.windows(2).all(|p| p[0] == p[1]), "{bodies:?}");
    // A non-Bearer scheme is refused too.
    let req = Request::builder()
        .uri("/api/v1/whoami")
        .header("authorization", format!("Basic {}", w.admin))
        .body(Body::empty())
        .unwrap();
    assert_eq!(router(w.s.clone()).oneshot(req).await.unwrap().status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tokens_are_shown_once_and_only_their_hash_is_kept() {
    let w = world();
    let c = w.s.authenticate(&w.admin).unwrap();
    let (status, created, _) = call(&w.s, "POST", "/api/v1/tokens", Some(&w.admin), Some(json!({"name": "ci"}))).await;
    assert_eq!(status, StatusCode::CREATED);
    let token = created["token"].as_str().unwrap();
    assert!(token.starts_with("mvx_") && token.len() == 68, "{token}");
    let id = created["details"]["id"].as_str().unwrap();
    let row = w.s.registry().token(id).unwrap().unwrap();
    assert_ne!(row.hash, token);
    assert!(!format!("{row:?}").contains(token));
    let (_, listed, _) = call(&w.s, "GET", "/api/v1/tokens", Some(&w.admin), None).await;
    assert!(!listed.to_string().contains(token));
    assert!(!listed.to_string().contains(&row.hash));
    // last used, scopes and expiry are shown.
    let mine = listed["items"].as_array().unwrap().iter().find(|t| t["name"] == "bootstrap").unwrap().clone();
    assert!(mine["last_used_at_ms"].is_number(), "{mine}");
    let _ = c;
}

#[tokio::test]
async fn bootstrap_happens_once() {
    let s = server();
    s.bootstrap("root").unwrap();
    assert!(matches!(s.bootstrap("again"), Err(memex_core::ServerError::AlreadyBootstrapped)));
}

#[tokio::test]
async fn a_vault_admin_can_not_mint_a_token_for_a_server_admin() {
    let w = world();
    let vault_admin = token_with(&w, "va", Some(Scope::Admin));
    let root_id = w.s.authenticate(&w.admin).unwrap().principal.id;
    let body = json!({"name": "steal", "principal_id": root_id, "grants": [{"vault_id": w.a.0, "scope": "read"}]});
    let (status, _, _) = call(&w.s, "POST", "/api/v1/tokens", Some(&vault_admin), Some(body)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // And a server admin can't lock themselves out by disabling themselves.
    assert_eq!(
        call(&w.s, "POST", &format!("/api/v1/principals/{root_id}/disable"), Some(&w.admin), None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn oversized_and_malformed_bodies_are_refused_in_the_one_format() {
    let w = world();
    let big = json!({"name": "x".repeat(BODY_LIMIT + 10)});
    let (status, body, _) = call(&w.s, "POST", "/api/v1/vaults", Some(&w.admin), Some(big)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"]["code"], "too_large");
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/vaults")
        .header("authorization", format!("Bearer {}", w.admin))
        .header("content-type", "application/json")
        .body(Body::from("{not json"))
        .unwrap();
    let res = router(w.s.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let (status, body, _) = call(&w.s, "POST", "/api/v1/vaults", Some(&w.admin), Some(json!({"name": "   "}))).await;
    assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid")));
    assert_eq!(
        call(&w.s, "POST", "/api/v1/vaults", Some(&w.admin), Some(json!({"name": "vault-a"}))).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(call(&w.s, "GET", "/nope", None, None).await.1["error"]["code"], "not_found");
    assert_eq!(call(&w.s, "PUT", "/api/v1/whoami", None, None).await.0, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn health_and_version_need_no_token_and_every_response_has_a_request_id() {
    let w = world();
    let (status, body, headers) = call(&w.s, "GET", "/api/v1/health", None, None).await;
    assert_eq!((status, body["status"].as_str()), (StatusCode::OK, Some("ok")));
    assert!(headers.get("x-request-id").is_some());
    assert_eq!(call(&w.s, "GET", "/api/v1/version", None, None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn lists_page_with_a_cursor_in_creation_order() {
    let w = world();
    let c = w.s.authenticate(&w.admin).unwrap();
    for i in 0..5 {
        w.s.engine().clock().advance(Duration::from_millis(1));
        w.s.create_vault(&c, &format!("v{i}")).unwrap();
    }
    let t = Some(w.admin.as_str());
    let (_, first, _) = call(&w.s, "GET", "/api/v1/vaults?limit=3&sort=created", t, None).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 3);
    let cursor = first["next_cursor"].as_str().unwrap().to_string();
    let (_, rest, _) = call(&w.s, "GET", &format!("/api/v1/vaults?limit=10&cursor={cursor}"), t, None).await;
    assert_eq!(rest["items"].as_array().unwrap().len(), 4);
    assert!(rest["next_cursor"].is_null());
    let all: Vec<i64> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(rest["items"].as_array().unwrap())
        .map(|v| v["created_at_ms"].as_i64().unwrap())
        .collect();
    assert!(all.windows(2).all(|p| p[0] <= p[1]), "{all:?}");
    for bad in ["limit=0", "limit=201", "sort=name"] {
        assert_eq!(
            call(&w.s, "GET", &format!("/api/v1/vaults?{bad}"), t, None).await.0,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
}

#[tokio::test]
async fn the_proposal_flow_over_http() {
    let w = world();
    let a = w.a.0.clone();
    let t = w.admin.clone();
    let t = Some(t.as_str());
    let page = |key: &str, ops: Value| json!({"ops": ops, "rationale": "because", "idempotency_key": key});
    let create = json!([{"op": "create_page", "id": "p1", "title": "T", "body": "B"}]);
    let base = format!("/api/v1/vaults/{a}");
    // Dry run first: nothing is written.
    let (status, dry, _) = call(&w.s, "POST", &format!("{base}/proposals?dry_run=true"), t, Some(page("k1", create.clone()))).await;
    assert_eq!(
        (status, dry["dry_run"].as_bool(), dry["risk"].as_str()),
        (StatusCode::OK, Some(true), Some("auto_commit"))
    );
    assert!(dry["changeset"].is_null());
    let (_, listed, _) = call(&w.s, "GET", &format!("{base}/changesets"), t, None).await;
    assert!(listed["items"].as_array().unwrap().is_empty());
    // Real, then the same key again.
    let (_, real, _) = call(&w.s, "POST", &format!("{base}/proposals"), t, Some(page("k1", create.clone()))).await;
    let cs = real["changeset"]["id"].as_str().unwrap().to_string();
    assert_eq!(real["changeset"]["state"], "committed");
    let (_, again, _) = call(&w.s, "POST", &format!("{base}/proposals"), t, Some(page("k1", create))).await;
    assert_eq!(
        (again["replayed"].as_bool(), again["changeset"]["id"].as_str()),
        (Some(true), Some(cs.as_str()))
    );
    // A stale edit gets a structured conflict with the current row.
    let edit = |key: &str, v: u32| page(key, json!([{"op": "edit_page", "id": "p1", "base_version": v, "body": "new"}]));
    assert_eq!(call(&w.s, "POST", &format!("{base}/proposals"), t, Some(edit("k2", 1))).await.0, StatusCode::OK);
    let (status, stale, _) = call(&w.s, "POST", &format!("{base}/proposals"), t, Some(edit("k3", 1))).await;
    assert_eq!((status, stale["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("stale_version")));
    assert_eq!(stale["error"]["current"]["page"]["header"]["version"], 2);
    // Deletes go to the queue, and need approval.
    let (_, del, _) = call(
        &w.s,
        "POST",
        &format!("{base}/proposals"),
        t,
        Some(page("k4", json!([{"op": "delete_page", "id": "p1", "base_version": 2}]))),
    )
    .await;
    assert_eq!(del["risk"], "review");
    let del_id = del["changeset"]["id"].as_str().unwrap().to_string();
    let (_, queue, _) = call(&w.s, "GET", &format!("{base}/changesets?state=pending"), t, None).await;
    assert_eq!(queue["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/changesets?state=nonsense"), t, None).await.0,
        StatusCode::BAD_REQUEST
    );
    let reader = token_with(&w, "r", Some(Scope::Read));
    assert_eq!(
        call(&w.s, "POST", &format!("{base}/changesets/{del_id}/approve"), Some(&reader), Some(json!({})))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/changesets/{del_id}"), Some(&reader), None).await.0,
        StatusCode::OK
    );
    let (_, done, _) = call(&w.s, "POST", &format!("{base}/changesets/{del_id}/approve"), t, Some(json!({"reason": "fine"}))).await;
    assert_eq!((done["state"].as_str(), done["decision_reason"].as_str()), (Some("committed"), Some("fine")));
    // Preview, then revert, then a second revert is refused.
    let (_, preview, _) = call(&w.s, "GET", &format!("{base}/changesets/{del_id}/inverse"), t, None).await;
    assert_eq!(preview["reversible"], true);
    assert_eq!(
        call(&w.s, "POST", &format!("{base}/changesets/{del_id}/revert"), t, Some(json!({"reason": "oops"})))
            .await
            .0,
        StatusCode::OK
    );
    let (status, second, _) = call(&w.s, "POST", &format!("{base}/changesets/{del_id}/revert"), t, Some(json!({}))).await;
    assert_eq!((status, second["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("not_reversible")));
    // Changes page by sequence.
    let (_, first, _) = call(&w.s, "GET", &format!("{base}/changes?limit=2"), t, None).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    let next = first["next_cursor"].as_str().unwrap();
    let (_, more, _) = call(&w.s, "GET", &format!("{base}/changes?cursor={next}"), t, None).await;
    assert_eq!(more["items"][0]["seq"], 2);
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/changes?cursor=abc"), t, None).await.0,
        StatusCode::BAD_REQUEST
    );
    // Other vaults stay hidden from a reader of this one.
    let b = w.b.0.clone();
    assert_eq!(
        call(&w.s, "GET", &format!("/api/v1/vaults/{b}/changesets"), Some(&reader), None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/contradictions"), Some(&reader), None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/assertions/as-of?subject=x&seq=0"), Some(&reader), None)
            .await
            .0,
        StatusCode::OK
    );
}

fn schema_v1() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["name"], "properties": {
        "name": {"type": "string", "identity": true, "searchable": true, "filterable": true},
        "price": {"type": "number", "filterable": true},
        "made": {"type": "string", "format": "date", "filterable": true},
        "notes": {"type": "string", "searchable": true}
    }})
}

async fn entity_setup(w: &World) -> (String, String) {
    let base = format!("/api/v1/vaults/{}", w.a.0);
    let (status, out, _) = call(
        &w.s,
        "POST",
        &format!("{base}/entity-types"),
        Some(&w.admin),
        Some(json!({"name": "Widget", "schema": schema_v1(), "rationale": "r", "idempotency_key": "t1"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["changeset"]["state"], "committed", "{out}");
    let seg = {
        let (_, up) = upload_raw(&w.s, &w.a.0, Some(&w.admin), "name=w.pdf", b"%PDF-1.7 pretend widget sheet".to_vec()).await;
        let sid = up["source"]["id"].as_str().unwrap().to_string();
        let source = memex_types::ObjectId(sid.clone());
        let job = w.s.claim_job(&w.a, "rendition", "w").unwrap().unwrap();
        let rows = w.s.build_rendition(&w.a, &source, memex_core::RenditionInput::Pdf(pdf_doc(), vec![])).unwrap();
        assert!(w.s.complete_job(&w.a, &job, rows).unwrap());
        let (_, rs, _) = call(&w.s, "GET", &format!("{base}/sources/{sid}/renditions"), Some(&w.admin), None).await;
        let rid = rs["items"][0]["id"].as_str().unwrap().to_string();
        let (_, segs, _) = call(&w.s, "GET", &format!("{base}/renditions/{rid}/segments"), Some(&w.admin), None).await;
        segs["items"][0]["id"].as_str().unwrap().to_string()
    };
    (base, seg)
}

async fn new_widget(w: &World, base: &str, key: &str, name: &str, seg: &str, price: f64) -> String {
    let (status, out, _) = call(
        &w.s,
        "POST",
        &format!("{base}/entities"),
        Some(&w.admin),
        Some(json!({
            "type": "Widget", "aliases": [format!("{name} alias")], "rationale": "r", "idempotency_key": key,
            "fields": [
                {"field": "name", "value": name, "evidence": [{"segment_id": seg, "quote": "Staden ligger"}]},
                {"field": "price", "value": price, "evidence": [{"segment_id": seg, "quote": "vid kusten"}]}
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["changeset"]["state"], "committed", "{out}");
    out["entity_id"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn every_field_of_a_returned_entity_leads_to_a_source_region() {
    let w = world();
    let (base, seg) = entity_setup(&w).await;
    let t = Some(w.admin.as_str());
    let id = new_widget(&w, &base, "w1", "Pump", &seg, 12.5).await;
    let (status, e, _) = call(&w.s, "GET", &format!("{base}/entities/{id}"), t, None).await;
    assert_eq!(status, StatusCode::OK, "{e}");
    assert_eq!(e["fields"].as_array().unwrap().len(), 2);
    for f in e["fields"].as_array().unwrap() {
        let claim = &f["current"];
        let evidence = claim["evidence"].as_array().unwrap();
        assert_eq!(evidence.len(), 1, "{f}");
        assert_eq!(evidence[0]["source_name"], "w.pdf");
        assert_eq!(evidence[0]["page_from"], 0);
        let aid = claim["assertion_id"].as_str().unwrap();
        let (status, lineage, _) = call(&w.s, "GET", &format!("{base}/assertions/{aid}/lineage"), t, None).await;
        assert_eq!(status, StatusCode::OK, "{lineage}");
        let region = &lineage["evidence"][0];
        assert_eq!(region["broken"], false);
        assert!(!region["rects"].as_array().unwrap().is_empty(), "the region has rectangles: {region}");
        let eid = evidence[0]["evidence_id"].as_str().unwrap();
        let (status, highlight, _) = call(&w.s, "GET", &format!("{base}/assertions/{aid}/evidence/{eid}/highlight"), t, None).await;
        assert_eq!(status, StatusCode::OK, "{highlight}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_sources_disagree_and_both_show_with_the_choice_and_why() {
    let w = world();
    let (base, seg) = entity_setup(&w).await;
    let t = Some(w.admin.as_str());
    let id = new_widget(&w, &base, "w1", "Pump", &seg, 10.0).await;
    let (status, out, _) = call(
        &w.s,
        "POST",
        &format!("{base}/entities/{id}/edit"),
        t,
        Some(json!({"base_version": 1, "fields": [{"field": "price", "value": 20.0, "evidence": [{"segment_id": seg, "quote": "Staden"}]}], "rationale": "second source", "idempotency_key": "w2"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["changeset"]["state"], "committed", "{out}");
    assert_eq!(out["contradictions"].as_array().unwrap().len(), 1);
    let (_, e, _) = call(&w.s, "GET", &format!("{base}/entities/{id}"), t, None).await;
    let price = e["fields"].as_array().unwrap().iter().find(|f| f["field"] == "price").unwrap();
    assert_eq!(price["value"], 20.0);
    assert!(price["why"].as_str().unwrap().contains("newer"), "{price}");
    assert_eq!(price["conflicts"].as_array().unwrap().len(), 1);
    assert_eq!(price["conflicts"][0]["value"], 10.0);
    assert_eq!(price["conflicts"][0]["evidence"][0]["quote"], "vid kusten");
    let (_, list, _) = call(&w.s, "GET", &format!("{base}/entities?type=Widget"), t, None).await;
    assert_eq!(list["items"][0]["conflicted_fields"], json!(["price"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn typed_filters_find_entities_and_bad_ones_say_why() {
    let w = world();
    let (base, seg) = entity_setup(&w).await;
    let t = Some(w.admin.as_str());
    let cheap = new_widget(&w, &base, "w1", "Pump", &seg, 10.0).await;
    let dear = new_widget(&w, &base, "w2", "Valve", &seg, 500.0).await;
    let ids = |v: &Value| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    let (_, all, _) = call(&w.s, "GET", &format!("{base}/entities?type=Widget"), t, None).await;
    assert_eq!(ids(&all), [cheap.clone(), dear.clone()]);
    let (_, under, _) = call(&w.s, "GET", &format!("{base}/entities?type=Widget&filter=price:lt:100"), t, None).await;
    assert_eq!(ids(&under), [cheap.clone()]);
    let (_, exact, _) = call(&w.s, "GET", &format!("{base}/entities?type=Widget&filter=name:eq:VALVE"), t, None).await;
    assert_eq!(ids(&exact), [dear.clone()], "text compares folded");
    let (_, both, _) = call(
        &w.s,
        "GET",
        &format!("{base}/entities?type=Widget&filter=price:gte:10&filter=name:eq:pump"),
        t,
        None,
    )
    .await;
    assert_eq!(ids(&both), [cheap.clone()]);
    let (_, page, _) = call(&w.s, "GET", &format!("{base}/entities?type=Widget&limit=1"), t, None).await;
    assert_eq!(ids(&page), [cheap.clone()]);
    let cursor = page["next_cursor"].as_str().unwrap();
    let (_, rest, _) = call(&w.s, "GET", &format!("{base}/entities?type=Widget&limit=1&cursor={cursor}"), t, None).await;
    assert_eq!(ids(&rest), [dear.clone()]);
    for bad in [
        "filter=price:lt:cheap",
        "filter=notes:eq:x",
        "filter=nope:eq:x",
        "filter=price%3C100",
        "filter=name:lt:x",
        "nope=1",
    ] {
        let (status, body, _) = call(&w.s, "GET", &format!("{base}/entities?type=Widget&{bad}"), t, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    assert_eq!(call(&w.s, "GET", &format!("{base}/entities?type=Nope"), t, None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_preview_matches_what_a_submission_does() {
    let w = world();
    let base = format!("/api/v1/vaults/{}", w.a.0);
    let t = Some(w.admin.as_str());
    let v1 = schema_v1();
    let mut additive = v1.clone();
    additive["properties"]["color"] = json!({"type": "string", "searchable": true});
    let mut breaking = additive.clone();
    breaking["properties"]["price"] = json!({"type": "string"});
    let mut remote = additive.clone();
    remote["properties"]["x"] = json!({"$ref": "https://example.com/s.json"});
    let cases = [
        ("Fresh", None, v1.clone(), "new", "auto_commit", "committed"),
        ("Fresh", Some(1), additive.clone(), "additive", "auto_commit", "committed"),
        ("Fresh", Some(2), breaking.clone(), "breaking", "review", "pending"),
        ("Fresh", Some(2), remote.clone(), "breaking", "reject", "rejected"),
        ("Fresh", Some(2), additive.clone(), "additive", "reject", "rejected"),
        ("Product", Some(1), json!({"type": "object"}), "breaking", "reject", "rejected"),
        ("Other", None, json!({"type": "object"}), "new", "reject", "rejected"),
    ];
    for (i, (name, base_version, schema, change, decision, state)) in cases.into_iter().enumerate() {
        let (status, p, _) = call(&w.s, "POST", &format!("{base}/entity-types/{name}/preview"), t, Some(json!({"schema": schema}))).await;
        assert_eq!(status, StatusCode::OK, "{p}");
        assert_eq!(p["change"], change, "case {i}: {p}");
        assert_eq!(p["decision"], decision, "case {i}: {p}");
        let (status, out, _) = call(
            &w.s,
            "POST",
            &format!("{base}/entity-types"),
            t,
            Some(json!({"name": name, "base_version": base_version, "schema": schema, "rationale": "r", "idempotency_key": format!("k{i}")})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{out}");
        assert_eq!(out["risk"], decision, "case {i}: {out}");
        assert_eq!(out["changeset"]["state"], state, "case {i}: {out}");
    }
    let (_, history, _) = call(&w.s, "GET", &format!("{base}/entity-types/Fresh/versions"), t, None).await;
    let changes: Vec<&str> = history["items"].as_array().unwrap().iter().map(|h| h["change"].as_str().unwrap()).collect();
    assert_eq!(changes, ["new", "additive"], "the breaking one is still waiting");
    let (_, types, _) = call(&w.s, "GET", &format!("{base}/entity-types"), t, None).await;
    let names: Vec<&str> = types["items"].as_array().unwrap().iter().map(|h| h["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Decision", "Fresh", "Organization", "Person", "Product", "Project"]);
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/entity-types/Nope/versions"), t, None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_and_split_wait_for_review_redirect_and_revert() {
    let w = world();
    let (base, seg) = entity_setup(&w).await;
    let t = Some(w.admin.as_str());
    let a = new_widget(&w, &base, "w1", "Pump", &seg, 10.0).await;
    let b = new_widget(&w, &base, "w2", "pump", &seg, 20.0).await;
    let (_, same, _) = call(&w.s, "GET", &format!("{base}/entities/same-as?type=Widget"), t, None).await;
    let groups = same["items"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "the shared name and alias are suggestions: {same}");
    assert_eq!(groups[0]["entity_ids"], json!([a.clone(), b.clone()]));
    let before = call(&w.s, "GET", &format!("{base}/entities/{b}"), t, None).await.1;
    let (_, m, _) = call(
        &w.s,
        "POST",
        &format!("{base}/entities/{a}/merge"),
        t,
        Some(json!({"into": b, "rationale": "the same pump", "idempotency_key": "m1"})),
    )
    .await;
    assert_eq!(m["changeset"]["state"], "pending", "{m}");
    let cs = m["changeset"]["id"].as_str().unwrap();
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/entities/{a}/redirect"), t, None).await.1["current_id"],
        a.as_str()
    );
    let (status, approved, _) = call(&w.s, "POST", &format!("{base}/changesets/{cs}/approve"), t, Some(json!({"reason": "yes"}))).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    let (_, r, _) = call(&w.s, "GET", &format!("{base}/entities/{a}/redirect"), t, None).await;
    assert_eq!(r["current_id"], b.as_str());
    assert_eq!(r["chain"], json!([a.clone()]));
    let (_, old, _) = call(&w.s, "GET", &format!("{base}/entities/{a}"), t, None).await;
    assert_eq!(old["id"], b.as_str());
    assert_eq!(old["redirected_from"], a.as_str());
    let (_, same, _) = call(&w.s, "GET", &format!("{base}/entities/same-as?type=Widget"), t, None).await;
    assert_eq!(same["items"].as_array().unwrap().len(), 0, "merged, so nothing is left to suggest: {same}");
    let (status, rev, _) = call(&w.s, "POST", &format!("{base}/changesets/{cs}/revert"), t, Some(json!({"reason": "no"}))).await;
    assert_eq!(status, StatusCode::OK, "{rev}");
    let mut after = call(&w.s, "GET", &format!("{base}/entities/{b}"), t, None).await.1;
    let mut was = before.clone();
    for v in [&mut after, &mut was] {
        v.as_object_mut().unwrap().remove("updated_at_ms");
    }
    assert_eq!(after, was, "a revert restores the state");
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/entities/{a}/redirect"), t, None).await.1["current_id"],
        a.as_str()
    );
    // Split: the price moves to a new entity, which needs its own name.
    let price = before["fields"].as_array().unwrap().iter().find(|f| f["field"] == "price").unwrap()["current"]["assertion_id"].clone();
    let (status, s, _) = call(
        &w.s,
        "POST",
        &format!("{base}/entities/{b}/split"),
        t,
        Some(json!({"base_version": after["version"], "moved": [price], "aliases": ["pump alias"], "fields": [{"field": "name", "value": "Another pump"}], "rationale": "two things", "idempotency_key": "s1"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{s}");
    assert_eq!(s["changeset"]["state"], "pending", "{s}");
    let cs = s["changeset"]["id"].as_str().unwrap();
    let new_id = s["entity_id"].as_str().unwrap();
    assert_eq!(call(&w.s, "GET", &format!("{base}/entities/{new_id}"), t, None).await.0, StatusCode::NOT_FOUND);
    let (status, approved, _) = call(&w.s, "POST", &format!("{base}/changesets/{cs}/approve"), t, Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    let (_, split_off, _) = call(&w.s, "GET", &format!("{base}/entities/{new_id}"), t, None).await;
    let fields = split_off["fields"].as_array().unwrap();
    assert_eq!(fields.len(), 2, "{split_off}");
    let moved = fields.iter().find(|f| f["field"] == "price").unwrap();
    assert_eq!(moved["value"], 20.0);
    assert_eq!(moved["current"]["evidence"][0]["quote"], "vid kusten", "evidence moves with the claim");
    let (_, stayed, _) = call(&w.s, "GET", &format!("{base}/entities/{b}"), t, None).await;
    assert!(stayed["fields"].as_array().unwrap().iter().all(|f| f["field"] != "price"), "{stayed}");
}

#[tokio::test(flavor = "multi_thread")]
async fn scopes_hold_on_entity_routes() {
    let w = world();
    let (base, seg) = entity_setup(&w).await;
    let id = new_widget(&w, &base, "w1", "Pump", &seg, 10.0).await;
    let reader = token_with(&w, "ent-r", Some(Scope::Read));
    let outsider = token_with(&w, "ent-none", None);
    let write = json!({"type": "Widget", "fields": [{"field": "name", "value": "x"}], "rationale": "r", "idempotency_key": "k"});
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/entities?type=Widget"), Some(&reader), None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&w.s, "POST", &format!("{base}/entities"), Some(&reader), Some(write.clone())).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/entities/{id}"), Some(&outsider), None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(call(&w.s, "GET", &format!("{base}/entity-types"), None, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        call(
            &w.s,
            "POST",
            &format!("{base}/entity-types/Widget/preview"),
            Some(&reader),
            Some(json!({"schema": {}}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[test]
fn every_list_route_in_the_openapi_document_has_cursor_limit_and_sort() {
    let doc = serde_json::to_value(openapi()).unwrap();
    let paths = doc["paths"].as_object().unwrap();
    let mut lists = 0;
    for (path, item) in paths {
        for (method, op) in item.as_object().unwrap() {
            let schema = op["responses"]["200"]["content"]["application/json"]["schema"]["$ref"].as_str().unwrap_or("");
            if schema.ends_with("Page") {
                lists += 1;
                let names: Vec<&str> = op["parameters"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
                for need in ["cursor", "limit", "sort"] {
                    assert!(names.contains(&need), "{method} {path} lacks {need}");
                }
            }
        }
    }
    assert_eq!(
        lists, 16,
        "vaults, principals, tokens, changesets, changes, contradictions, sources, jobs, segments, re-anchor failures, flagged pages, two claim lists, entity types, entities and same-as suggestions"
    );
    // Every route the router serves is documented.
    for p in [
        "/api/v1/health",
        "/api/v1/version",
        "/api/v1/whoami",
        "/api/v1/vaults/{id}/entity-types",
        "/api/v1/vaults/{id}/entity-types/{name}/versions",
        "/api/v1/vaults/{id}/entity-types/{name}/preview",
        "/api/v1/vaults/{id}/entities",
        "/api/v1/vaults/{id}/entities/resolve",
        "/api/v1/vaults/{id}/entities/same-as",
        "/api/v1/vaults/{id}/entities/{eid}",
        "/api/v1/vaults/{id}/entities/{eid}/edit",
        "/api/v1/vaults/{id}/entities/{eid}/redirect",
        "/api/v1/vaults/{id}/entities/{eid}/merge",
        "/api/v1/vaults/{id}/entities/{eid}/split",
        "/api/v1/vaults",
        "/api/v1/vaults/{id}",
        "/api/v1/principals",
        "/api/v1/principals/{id}/disable",
        "/api/v1/tokens",
        "/api/v1/tokens/{id}",
    ] {
        assert!(paths.contains_key(p), "{p}");
    }
}

async fn upload_raw(s: &Arc<Server<Fakes>>, vault: &str, token: Option<&str>, query: &str, bytes: Vec<u8>) -> (StatusCode, Value) {
    let mut req = Request::builder().method("POST").uri(format!("/api/v1/vaults/{vault}/sources?{query}"));
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let res = router(s.clone()).oneshot(req.body(Body::from(bytes)).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 4 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_jobs_and_sources_over_http() {
    let w = world();
    let a = w.a.0.clone();
    let t = w.admin.clone();
    let t = Some(t.as_str());
    let base = format!("/api/v1/vaults/{a}");
    // A body bigger than the JSON limit goes through, as uploads have their own limit.
    let mut pdf = b"%PDF-1.7 ".to_vec();
    pdf.resize(BODY_LIMIT + 100, b'x');
    let (status, first) = upload_raw(&w.s, &a, t, "name=report.pdf", pdf.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!(
        (first["source"]["media_type"].as_str(), first["existing"].as_bool()),
        (Some("application/pdf"), Some(false))
    );
    let (sid, jid) = (
        first["source"]["id"].as_str().unwrap().to_string(),
        first["job_id"].as_str().unwrap().to_string(),
    );
    // Same bytes again.
    let (status, again) = upload_raw(&w.s, &a, t, "name=copy.pdf", pdf).await;
    assert_eq!(
        (status, again["existing"].as_bool(), again["source"]["id"].as_str()),
        (StatusCode::OK, Some(true), Some(sid.as_str()))
    );
    // Bad uploads.
    assert_eq!(
        upload_raw(&w.s, &a, t, "name=a.bin", vec![0, 1, 2, 3]).await.0,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(upload_raw(&w.s, &a, t, "name=a.txt", vec![]).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        upload_raw(&w.s, &a, t, "name=a.txt&trust_tier=bogus", b"text".to_vec()).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(upload_raw(&w.s, &a, None, "name=a.txt", b"text".to_vec()).await.0, StatusCode::UNAUTHORIZED);
    let reader = token_with(&w, "r", Some(Scope::Read));
    assert_eq!(
        upload_raw(&w.s, &a, Some(&reader), "name=a.txt", b"text".to_vec()).await.0,
        StatusCode::FORBIDDEN
    );
    // Reads.
    let (_, limits, _) = call(&w.s, "GET", &format!("{base}/upload-limits"), Some(&reader), None).await;
    assert!(limits["allowed_media_types"].as_array().unwrap().iter().any(|m| m == "application/pdf"));
    let (_, sources, _) = call(&w.s, "GET", &format!("{base}/sources"), Some(&reader), None).await;
    assert_eq!(sources["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/sources/{sid}"), Some(&reader), None).await.1["name"],
        "report.pdf"
    );
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/sources/nope"), Some(&reader), None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/sources/{sid}/versions"), Some(&reader), None).await.1["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Jobs.
    let (_, jobs, _) = call(&w.s, "GET", &format!("{base}/jobs?kind=rendition&state=queued"), Some(&reader), None).await;
    assert_eq!(jobs["items"][0]["id"].as_str(), Some(jid.as_str()));
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/jobs?state=dead"), Some(&reader), None).await.1["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/jobs?state=nonsense"), Some(&reader), None).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(call(&w.s, "GET", &format!("{base}/jobs/{jid}"), Some(&reader), None).await.1["state"], "queued");
    assert_eq!(
        call(&w.s, "POST", &format!("{base}/jobs/{jid}/cancel"), Some(&reader), None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(call(&w.s, "POST", &format!("{base}/jobs/{jid}/retry"), t, None).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&w.s, "POST", &format!("{base}/jobs/{jid}/cancel"), t, None).await.1["state"], "cancelled");
    assert_eq!(call(&w.s, "POST", &format!("{base}/jobs/{jid}/retry"), t, None).await.1["state"], "queued");
    // The other vault stays hidden.
    let b = w.b.0.clone();
    assert_eq!(
        upload_raw(&w.s, &b, Some(&reader), "name=a.txt", b"text".to_vec()).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&w.s, "GET", &format!("/api/v1/vaults/{b}/jobs"), Some(&reader), None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn over_the_upload_limit_gives_413() {
    let w = world();
    let cap = memex_core::UploadLimits {
        max_bytes: 10,
        max_open_jobs: 10,
    };
    let c = w.s.authenticate(&w.admin).unwrap();
    let out = w.s.upload(
        &c,
        &w.a,
        memex_core::UploadRequest {
            name: "a.txt".into(),
            trust_tier: "unverified".into(),
            metadata: json!({}),
            replaces: None,
        },
        cap,
        &mut &[b'x'; 11][..],
    );
    assert!(matches!(out, Err(memex_core::ServerError::TooLarge { limit: 10 })));
}

#[tokio::test(flavor = "multi_thread")]
async fn rendition_routes_show_what_the_pipeline_stored() {
    let w = world();
    let a = w.a.0.clone();
    let t = w.admin.clone();
    let t = Some(t.as_str());
    let base = format!("/api/v1/vaults/{a}");
    let (_, up) = upload_raw(&w.s, &a, t, "name=notes.md", b"# Titel\n\nStaden ligger vid Gota alv.".to_vec()).await;
    let sid = up["source"]["id"].as_str().unwrap().to_string();
    let job = w.s.claim_job(&w.a, "rendition", "w").unwrap().unwrap();
    let rows =
        w.s.build_rendition(
            &w.a,
            &memex_types::ObjectId(sid.clone()),
            memex_core::RenditionInput::Plain {
                text: "# Titel\n\nStaden ligger vid Gota alv.".into(),
                parser: "plain-1".into(),
            },
        )
        .unwrap();
    assert!(w.s.complete_job(&w.a, &job, rows).unwrap());
    let reader = token_with(&w, "r", Some(Scope::Read));
    let (_, list, _) = call(&w.s, "GET", &format!("{base}/sources/{sid}/renditions"), Some(&reader), None).await;
    let rid = list["items"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(list["items"][0]["segment_count"], 1);
    let (_, detail, _) = call(&w.s, "GET", &format!("{base}/renditions/{rid}"), Some(&reader), None).await;
    assert!(detail["canonical_text"].as_str().unwrap().contains("Staden ligger"));
    let (_, segs, _) = call(&w.s, "GET", &format!("{base}/renditions/{rid}/segments"), Some(&reader), None).await;
    let seg = segs["items"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(segs["items"][0]["heading"], "Titel");
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/segments/{seg}"), Some(&reader), None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/segments/nope"), Some(&reader), None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/reanchor-failures"), Some(&reader), None).await.1["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        call(&w.s, "POST", &format!("{base}/sources/{sid}/reprocess"), Some(&reader), None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&w.s, "POST", &format!("{base}/sources/{sid}/reprocess"), t, None).await.1["state"],
        "queued"
    );
    assert_eq!(
        call(&w.s, "GET", &format!("/api/v1/vaults/{}/renditions/{rid}", w.b.0), Some(&reader), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

fn claim_op(id: &str, segment: &str, quote: &str) -> Value {
    json!({"op": "add_assertion", "id": id, "subject": format!("s{id}"), "predicate": "says", "value": quote, "epistemic": "asserted", "machine": false, "confidence_permille": 900, "valid_from_ms": null, "valid_to_ms": null,
        "evidence": [[format!("{id}-e"), {"segment_id": segment, "quote": quote, "stance": "supports"}]]})
}

fn pdf_doc() -> memex_types::ParsedDocument {
    let word = |i: usize, text: &str| memex_types::Word {
        text: text.into(),
        page: 0,
        rect: memex_types::Rect {
            x0: 1000 + i as i32 * 3000,
            y0: 1000,
            x1: 3500 + i as i32 * 3000,
            y1: 2000,
        },
        confidence: None,
        quad: None,
    };
    memex_types::ParsedDocument {
        parser: "fake-1".into(),
        pages: vec![memex_types::PageFacts {
            index: 0,
            width: 60000,
            height: 80000,
            rotation: 0,
            class: memex_types::PageClass::Text,
            hidden_text_objects: 0,
        }],
        words: ["Staden", "ligger", "vid", "kusten"].iter().enumerate().map(|(i, t)| word(i, t)).collect(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn provenance_from_a_claim_to_the_region_and_back() {
    let w = world();
    let a = w.a.0.clone();
    let t = w.admin.clone();
    let t = Some(t.as_str());
    let base = format!("/api/v1/vaults/{a}");
    let bytes = b"%PDF-1.7 pretend, 0123456789".to_vec();
    let (_, up) = upload_raw(&w.s, &a, t, "name=r%20e.pdf", bytes.clone()).await;
    let sid = up["source"]["id"].as_str().unwrap().to_string();
    let source = memex_types::ObjectId(sid.clone());
    let job = w.s.claim_job(&w.a, "rendition", "w").unwrap().unwrap();
    let rows = w.s.build_rendition(&w.a, &source, memex_core::RenditionInput::Pdf(pdf_doc(), vec![])).unwrap();
    assert!(w.s.complete_job(&w.a, &job, rows).unwrap());
    let reader = token_with(&w, "prov-r", Some(Scope::Read));
    let (_, renditions, _) = call(&w.s, "GET", &format!("{base}/sources/{sid}/renditions"), t, None).await;
    let rid = renditions["items"][0]["id"].as_str().unwrap().to_string();
    let (_, segs, _) = call(&w.s, "GET", &format!("{base}/renditions/{rid}/segments"), t, None).await;
    let seg = segs["items"][0]["id"].as_str().unwrap().to_string();
    let (_, prop, _) = call(
        &w.s,
        "POST",
        &format!("{base}/proposals"),
        t,
        Some(json!({"ops": [claim_op("c1", &seg, "ligger vid"), claim_op("c2", &seg, "Staden")], "rationale": "r", "idempotency_key": "p1"})),
    )
    .await;
    assert_eq!(prop["changeset"]["state"], "committed", "{prop}");
    // Forward: the claim to the source, with rects and who proposed it.
    let (status, lineage, _) = call(&w.s, "GET", &format!("{base}/assertions/c1/lineage"), Some(&reader), None).await;
    assert_eq!(status, StatusCode::OK, "{lineage}");
    let e = &lineage["evidence"][0];
    assert_eq!(
        (e["source_id"].as_str(), e["broken"].as_bool(), e["superseded"].as_bool()),
        (Some(sid.as_str()), Some(false), Some(false))
    );
    assert_eq!((e["rects"][0]["x0"].as_i64(), e["rects"][0]["x1"].as_i64()), (Some(4000), Some(9500)));
    assert_eq!(e["parser"], "fake-1");
    assert!(lineage["proposer"].is_string());
    // The rects and the image agree on what's asked for.
    let (_, h, _) = call(&w.s, "GET", &format!("{base}/assertions/c1/evidence/c1-e/highlight"), Some(&reader), None).await;
    assert_eq!(
        (h["page"].as_u64(), h["page_width"].as_i64(), h["rects"].as_array().unwrap().len()),
        (Some(0), Some(60000), 1)
    );
    let req = |path: String, token: &str| {
        Request::builder()
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    };
    let res = router(w.s.clone())
        .oneshot(req(format!("{base}/assertions/c1/evidence/c1-e/highlight.png?dpi=72"), &reader))
        .await
        .unwrap();
    assert_eq!(res.headers()["content-type"], "image/png");
    assert_eq!(to_bytes(res.into_body(), 4096).await.unwrap(), "fake page 0 at 72 with 1 highlights");
    // Pages, thumbnails and refused values.
    let (_, pages, _) = call(&w.s, "GET", &format!("{base}/sources/{sid}/pages"), Some(&reader), None).await;
    assert_eq!((pages["page_count"].as_u64(), pages["pages"][0]["width"].as_i64()), (Some(1), Some(60000)));
    assert!(pages["allowed_dpi"].as_array().unwrap().iter().any(|d| d == 150));
    let thumb = router(w.s.clone())
        .oneshot(req(format!("{base}/sources/{sid}/pages/0/thumbnail"), &reader))
        .await
        .unwrap();
    assert_eq!(to_bytes(thumb.into_body(), 4096).await.unwrap(), "fake page 0 at 36 with 0 highlights");
    for bad in ["pages/5/image", "pages/0/image?dpi=9999", "pages/0/image?dpi=0"] {
        assert_eq!(
            call(&w.s, "GET", &format!("{base}/sources/{sid}/{bad}"), Some(&reader), None).await.0,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    // The original, whole and in ranges, checked against the bytes.
    let file = router(w.s.clone()).oneshot(req(format!("{base}/sources/{sid}/file"), &reader)).await.unwrap();
    assert_eq!(file.status(), StatusCode::OK);
    assert!(
        file.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment; filename=\"r_e.pdf\""),
        "{:?}",
        file.headers()
    );
    assert_eq!(file.headers()["x-content-type-options"], "nosniff");
    assert_eq!(to_bytes(file.into_body(), 4096).await.unwrap(), bytes);
    for (range, from, to) in [
        ("bytes=2-5", 2usize, 6usize),
        ("bytes=20-", 20, bytes.len()),
        ("bytes=-3", bytes.len() - 3, bytes.len()),
        ("bytes=0-99999", 0, bytes.len()),
    ] {
        let r = Request::builder()
            .uri(format!("{base}/sources/{sid}/file"))
            .header("authorization", format!("Bearer {reader}"))
            .header("range", range)
            .body(Body::empty())
            .unwrap();
        let res = router(w.s.clone()).oneshot(r).await.unwrap();
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT, "{range}");
        assert_eq!(res.headers()["content-range"], format!("bytes {from}-{}/{}", to - 1, bytes.len()), "{range}");
        assert_eq!(to_bytes(res.into_body(), 4096).await.unwrap().as_ref(), &bytes[from..to], "{range}");
    }
    assert_eq!(memex_http::parse_range("bytes=5-2", 10), None);
    assert_eq!(memex_http::parse_range("bytes=50-", 10), None);
    assert_eq!(memex_http::parse_range("items=1-2", 10), None);
    assert_eq!(memex_http::parse_range("bytes=1-2,4-5", 10), None);
    // Backward: a source and a segment give the claims forward lineage connects to them, and no others.
    let (_, by_source, _) = call(&w.s, "GET", &format!("{base}/sources/{sid}/claims"), Some(&reader), None).await;
    let ids = |v: &Value| {
        let mut x: Vec<String> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["assertion_id"].as_str().unwrap().to_string())
            .collect();
        x.sort();
        x
    };
    assert_eq!(ids(&by_source), vec!["c1", "c2"]);
    assert_eq!(
        ids(&call(&w.s, "GET", &format!("{base}/segments/{seg}/claims"), Some(&reader), None).await.1),
        vec!["c1", "c2"]
    );
    // A new rendition: the claim still resolves, on the new one, with the quote that's gone reported as broken, not guessed.
    let mut changed = pdf_doc();
    changed.parser = "fake-2".into();
    changed.words[0].text = "Byn".into();
    w.s.reprocess_source(&w.s.authenticate(&w.admin).unwrap(), &w.a, &source).unwrap();
    let job = w.s.claim_job(&w.a, "rendition", "w").unwrap().unwrap();
    let rows = w.s.build_rendition(&w.a, &source, memex_core::RenditionInput::Pdf(changed, vec![])).unwrap();
    assert!(w.s.complete_job(&w.a, &job, rows).unwrap());
    let (_, moved, _) = call(&w.s, "GET", &format!("{base}/assertions/c1/lineage"), Some(&reader), None).await;
    assert_eq!(
        (moved["evidence"][0]["broken"].as_bool(), moved["evidence"][0]["parser"].as_str()),
        (Some(false), Some("fake-2"))
    );
    // A quote gone from the new rendition still resolves on the old one, marked as both.
    let (_, gone, _) = call(&w.s, "GET", &format!("{base}/assertions/c2/lineage"), Some(&reader), None).await;
    assert_eq!(
        (gone["evidence"][0]["broken"].as_bool(), gone["evidence"][0]["superseded"].as_bool()),
        (Some(true), Some(true)),
        "{gone}"
    );
    assert_eq!(gone["evidence"][0]["rects"].as_array().unwrap().len(), 1);
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/assertions/c2/evidence/c2-e/highlight.png"), Some(&reader), None)
            .await
            .0,
        StatusCode::OK
    );
    // A quote that resolves nowhere is reported as broken, with nothing guessed.
    let admin = w.s.authenticate(&w.admin).unwrap();
    let snap_row = {
        use memex_core::{Snapshot, Store};
        w.s.engine()
            .store()
            .snapshot(&w.a)
            .unwrap()
            .get(&memex_types::RowKey {
                table: memex_types::Table::Evidence,
                id: memex_types::ObjectId("c1-e".into()),
            })
            .unwrap()
            .unwrap()
    };
    let memex_types::Row::Evidence(mut evidence) = snap_row else {
        panic!("evidence")
    };
    evidence.quote = "words that are nowhere in the document".into();
    w.s.apply_as(&admin, &w.a, "state.recorded", 1, json!({"mutations": [{"put": {"evidence": evidence}}]}))
        .unwrap();
    let (_, nowhere, _) = call(&w.s, "GET", &format!("{base}/assertions/c1/lineage"), Some(&reader), None).await;
    assert_eq!(
        (
            nowhere["evidence"][0]["broken"].as_bool(),
            nowhere["evidence"][0]["rects"].as_array().unwrap().len()
        ),
        (Some(true), 0),
        "{nowhere}"
    );
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/assertions/c1/evidence/c1-e/highlight.png"), Some(&reader), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    // A token for another vault sees nothing, on any of these.
    let b = w.b.0.clone();
    for path in [
        "assertions/c1/lineage".to_string(),
        format!("sources/{sid}/file"),
        format!("sources/{sid}/pages"),
        format!("sources/{sid}/claims"),
        format!("segments/{seg}/claims"),
    ] {
        assert_eq!(
            call(&w.s, "GET", &format!("/api/v1/vaults/{b}/{path}"), Some(&reader), None).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/assertions/nope/lineage"), Some(&reader), None).await.0,
        StatusCode::NOT_FOUND
    );
    // A text source has no pages to draw.
    let (_, text) = upload_raw(&w.s, &a, t, "name=a.txt", b"plain text here".to_vec()).await;
    let tid = text["source"]["id"].as_str().unwrap().to_string();
    let job = w.s.claim_job(&w.a, "rendition", "w").unwrap().unwrap();
    let rows =
        w.s.build_rendition(
            &w.a,
            &memex_types::ObjectId(tid.clone()),
            memex_core::RenditionInput::Plain {
                text: "plain text here".into(),
                parser: "plain-1".into(),
            },
        )
        .unwrap();
    assert!(w.s.complete_job(&w.a, &job, rows).unwrap());
    assert_eq!(
        call(&w.s, "GET", &format!("{base}/sources/{tid}/pages/0/image"), Some(&reader), None).await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn keyword_search_answers_refuses_and_stays_in_its_vault() {
    use memex_core::{Snapshot as _, StepOutcome};
    let w = world();
    let c = w.s.authenticate(&w.admin).unwrap();
    let url = |v: &VaultId, q: &str| format!("/api/v1/vaults/{}/search/lexical?{q}", v.0);

    // Nothing has been built yet, and that's not the caller's fault.
    let (status, body, _) = call(&w.s, "GET", &url(&w.a, "q=pump"), Some(&w.admin), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");

    w.s.record_segment(
        &c,
        &w.a,
        &memex_types::ObjectId("s1".into()),
        &memex_types::ObjectId("src".into()),
        None,
        "the pump ORA-00904",
    )
    .unwrap();
    w.s.record_segment(
        &c,
        &w.b,
        &memex_types::ObjectId("s2".into()),
        &memex_types::ObjectId("src".into()),
        None,
        "the pump elsewhere",
    )
    .unwrap();
    for v in [&w.a, &w.b] {
        let mut idle = 0;
        while idle < 2 {
            w.s.engine().clock().advance(std::time::Duration::from_secs(1));
            idle = if w.s.project_lexical(v).unwrap() == StepOutcome::Idle { idle + 1 } else { 0 };
        }
    }
    let (status, body, _) = call(&w.s, "GET", &url(&w.a, "q=pump"), Some(&w.admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["hits"].as_array().unwrap().len(), 1);
    assert_eq!(body["hits"][0]["id"], "s1");
    assert_eq!(body["hits"][0]["kind"], "segment");
    assert_eq!(body["watermark"], w.s.engine().store().snapshot(&w.a).unwrap().head().unwrap().unwrap());

    let (_, body, _) = call(&w.s, "GET", &url(&w.a, "q=ORA-00904&kind=claim"), Some(&w.admin), None).await;
    assert_eq!(body["hits"].as_array().unwrap().len(), 0, "the filter applies");

    for (q, needle) in [("q=%22open", "quote"), ("q=", "no words"), ("q=x&limit=0", "limit"), ("q=x&kind=nope", "kind")] {
        let (status, body, _) = call(&w.s, "GET", &url(&w.a, q), Some(&w.admin), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{q}: {body}");
        assert!(body["error"]["message"].as_str().unwrap().contains(needle), "{q}: {body}");
    }
    let (status, _, _) = call(&w.s, "GET", &url(&w.a, "q=pump"), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let reader = token_with(&w, "reader-b", Some(Scope::Read));
    let (status, _, _) = call(&w.s, "GET", &url(&w.b, "q=pump"), Some(&reader), None).await;
    assert!(status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND, "{status}");
}

#[tokio::test(flavor = "multi_thread")]
async fn search_returns_an_evidence_package_whose_citations_lead_back_to_the_text() {
    use memex_core::StepOutcome;
    let w = world();
    let root = w.admin.clone();
    let t = Some(root.as_str());
    let base = format!("/api/v1/vaults/{}", w.a.0);
    let (_, up) = upload_raw(&w.s, &w.a.0, t, "name=s.pdf", b"%PDF-1.7 search".to_vec()).await;
    let sid = up["source"]["id"].as_str().unwrap().to_string();
    let source = memex_types::ObjectId(sid.clone());
    let job = w.s.claim_job(&w.a, "rendition", "w").unwrap().unwrap();
    let rows = w.s.build_rendition(&w.a, &source, memex_core::RenditionInput::Pdf(pdf_doc(), vec![])).unwrap();
    assert!(w.s.complete_job(&w.a, &job, rows).unwrap());
    for _ in 0..200 {
        w.s.engine().clock().advance(Duration::from_secs(60));
        while w.s.project_lexical(&w.a).unwrap() == StepOutcome::Progress {}
        while w.s.project_vector(&w.a).unwrap() == StepOutcome::Progress {}
        let Some(embed) = w.s.claim_job(&w.a, memex_core::EMBED_JOB, "w").unwrap() else {
            break;
        };
        w.s.run_embed_job(&w.a, &embed).unwrap();
        w.s.complete_job(&w.a, &embed, vec![]).unwrap();
    }
    let (_, renditions, _) = call(&w.s, "GET", &format!("{base}/sources/{sid}/renditions"), t, None).await;
    let rid = renditions["items"][0]["id"].as_str().unwrap().to_string();
    let (status, body, _) = call(&w.s, "GET", &format!("{base}/search?q=Staden&debug=true&facets=true"), t, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["low_relevance"], false, "{body}");
    assert_eq!(body["path"], "natural");
    assert!(body["debug"]["signals"].is_array() && body["facets"]["trust_tier"].is_array(), "{body}");
    let item = body["items"].as_array().unwrap().iter().find(|i| i["kind"] == "segment").expect("a segment");
    let c = &item["citations"][0];
    assert_eq!(c["anchor"], "located");
    assert_eq!(c["source_id"], sid.as_str());
    assert_eq!(c["rendition_id"], rid.as_str());
    let caller = w.s.authenticate(&root).unwrap();
    let canonical = w.s.rendition_blob(&caller, &w.a, &memex_types::ObjectId(rid.clone()), false).unwrap();
    let (start, end) = (c["start"].as_u64().unwrap() as usize, c["end"].as_u64().unwrap() as usize);
    assert_eq!(
        std::str::from_utf8(&canonical[start..end]).unwrap(),
        item["text"].as_str().unwrap(),
        "the offsets lead to the same text"
    );

    let (_, plain, _) = call(&w.s, "GET", &format!("{base}/search?q=Staden"), t, None).await;
    assert!(plain.get("debug").is_none() && plain.get("facets").is_none(), "{plain}");

    for (q, needle) in [
        ("q=", "no words"),
        ("q=%22open", "quote"),
        ("q=x&mode=nope", "mode"),
        ("q=x&limit=0", "limit"),
        ("q=x&budget_tokens=0", "budget"),
        ("q=x&kind=nope", "kind"),
        ("q=x&cursor=nope", "cursor"),
        ("q=x&as_of=1&as_of_ms=1", "not both"),
        ("q=x&min_seq=99999999", "ahead of the log"),
        ("q=x&as_of=99999999", "ahead of the log"),
        ("q=x&min_seq_wait_ms=30001", "min_seq_wait_ms"),
    ] {
        let (status, body, _) = call(&w.s, "GET", &format!("{base}/search?{q}"), t, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{q}: {body}");
        assert!(body["error"]["message"].as_str().unwrap().contains(needle), "{q}: {body}");
    }

    let (status, past, _) = call(&w.s, "GET", &format!("{base}/search?q=Staden&as_of_ms=-1"), t, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(past["items"].as_array().unwrap().len(), 0);
    assert_eq!(past["low_relevance"], true);

    // Something written now and never projected: `min_seq` waits as long as it's allowed, then says so.
    let seq =
        w.s.record_segment(&caller, &w.a, &memex_types::ObjectId("late".into()), &source, None, "a late arrival")
            .unwrap()
            .seq;
    let (status, body, _) = call(&w.s, "GET", &format!("{base}/search?q=late&min_seq={seq}&min_seq_wait_ms=120"), t, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "not_caught_up");

    let (status, _, _) = call(&w.s, "GET", &format!("{base}/search?q=x"), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
