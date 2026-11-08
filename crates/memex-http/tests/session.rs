//! Browser sessions and the embedded console, in memory: real routing with fake edges.
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request, StatusCode};
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
use memex_core::{LOGIN_FAILURE_LIMIT, LOGIN_WINDOW_MS, NewToken, Registry, SESSION_TTL_MS, Server, ServerEdges};
use memex_http::{CSRF_HEADER, SESSION_COOKIE, chat_routes, openapi, router_with, router_with_console};
use memex_types::{PrincipalKind, VaultId};
use serde_json::{Value, json};
use std::borrow::Cow;
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

const INDEX: &[u8] = b"<!doctype html><title>the console</title>";
const SCRIPT: &[u8] = b"console.log(1)";

fn built_console(path: &str) -> Option<Cow<'static, [u8]>> {
    match path {
        "index.html" => Some(Cow::Borrowed(INDEX)),
        "assets/index-abc123.js" => Some(Cow::Borrowed(SCRIPT)),
        "robots.txt" => Some(Cow::Borrowed(b"no")),
        _ => None,
    }
}

fn no_console(_: &str) -> Option<Cow<'static, [u8]>> {
    None
}

struct World {
    s: Arc<Server<Fakes>>,
    admin: String,
    vault: VaultId,
}

fn world() -> World {
    let s = Arc::new(Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_700_000_000_000),
        SeededRandom::new(7),
        memex_core::testing::mem_search(),
    ));
    let admin = s.bootstrap("root").unwrap().token;
    let c = s.authenticate(&admin).unwrap();
    let vault = s.create_vault(&c, "v").unwrap().id;
    World { s, admin, vault }
}

impl World {
    fn app(&self) -> Router {
        // With the route the composition root adds, as the contract documents it.
        router_with(self.s.clone(), built_console, chat_routes::<Fakes, memex_core::testing::ScriptedChat>(None))
    }

    async fn send(&self, req: Request<Body>) -> (StatusCode, Vec<u8>, HeaderMap) {
        send(self.app(), req).await
    }

    /// An ordinary console request: the cookie and the custom header. Only the status is read, since a stream's body never ends.
    async fn with_cookie(&self, method: &str, path: &str, cookie: &str) -> StatusCode {
        let req = Request::builder().method(method).uri(path).header("cookie", cookie).header(CSRF_HEADER, "1");
        self.app().oneshot(req.body(Body::empty()).unwrap()).await.unwrap().status()
    }

    async fn login(&self, token: &str) -> (StatusCode, HeaderMap) {
        let (status, _, headers) = self.send(login_request(token)).await;
        (status, headers)
    }

    /// Signs in, and returns the value for a `Cookie` header.
    async fn cookie(&self, token: &str) -> String {
        let (status, headers) = self.login(token).await;
        assert_eq!(status, StatusCode::CREATED);
        headers["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string()
    }
}

async fn send(app: Router, req: Request<Body>) -> (StatusCode, Vec<u8>, HeaderMap) {
    let res = app.oneshot(req).await.unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    (status, to_bytes(res.into_body(), 4 << 20).await.unwrap().to_vec(), headers)
}

fn login_request(token: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/session")
        .header("content-type", "application/json")
        .header(CSRF_HEADER, "1")
        .body(Body::from(json!({ "token": token }).to_string()))
        .unwrap()
}

fn get(path: &str) -> Request<Body> {
    Request::builder().uri(path).body(Body::empty()).unwrap()
}

fn code(bytes: &[u8]) -> String {
    serde_json::from_slice::<Value>(bytes).unwrap()["error"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn signing_in_sets_a_locked_down_cookie_and_stores_only_a_hash() {
    let w = world();
    let (status, headers) = w.login(&w.admin).await;
    assert_eq!(status, StatusCode::CREATED);
    let cookie = headers["set-cookie"].to_str().unwrap();
    assert!(cookie.starts_with(&format!("{SESSION_COOKIE}=mxs_")), "{cookie}");
    for part in ["HttpOnly", "SameSite=Strict", "Path=/api/v1", "Max-Age=43200"] {
        assert!(cookie.contains(part), "{cookie} lacks {part}");
    }
    assert!(!cookie.contains("Secure"), "plain HTTP can't use a Secure cookie: {cookie}");
    assert_eq!(headers["cache-control"], "no-store");
    let id = cookie.split(';').next().unwrap().split_once('=').unwrap().1;
    assert_eq!(w.s.registry().session_by_hash(id).unwrap(), None, "the id itself isn't a key");
    assert!(!w.s.registry().fingerprint().contains(id), "the id itself is never stored");
    assert!(w.s.authenticate_session(id).is_ok());
    assert!(w.s.registry().audit_log(1).unwrap()[0].action == "session.create");
}

#[tokio::test]
async fn the_cookie_is_secure_behind_tls() {
    let w = world();
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/session")
        .header("content-type", "application/json")
        .header("x-forwarded-proto", "https")
        .header(CSRF_HEADER, "1")
        .body(Body::from(json!({ "token": w.admin }).to_string()))
        .unwrap();
    let (_, _, headers) = w.send(req).await;
    assert!(headers["set-cookie"].to_str().unwrap().ends_with("; Secure"));
}

#[tokio::test]
async fn whoami_says_the_same_for_a_cookie_and_a_bearer_token() {
    let w = world();
    let cookie = w.cookie(&w.admin).await;
    let (status, by_cookie, _) = w
        .send(Request::builder().uri("/api/v1/whoami").header("cookie", &cookie).body(Body::empty()).unwrap())
        .await;
    assert_eq!(status, StatusCode::OK);
    let bearer = format!("Bearer {}", w.admin);
    let (_, by_bearer, _) = w
        .send(
            Request::builder()
                .uri("/api/v1/whoami")
                .header("authorization", bearer)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(by_cookie, by_bearer);
}

/// Spec 037's goal is every route, including images, audio and streams, so this walks the generated contract and not a list that could go stale.
#[tokio::test]
async fn every_route_in_the_contract_takes_a_cookie_and_refuses_no_credentials() {
    let w = world();
    let cookie = w.cookie(&w.admin).await;
    let public = ["/api/v1/health", "/api/v1/version", "/api/v1/session"];
    let (mut checked, mut refused) = (0, 0);
    let doc = serde_json::to_value(openapi()).unwrap();
    for (template, item) in doc["paths"].as_object().unwrap() {
        let path = template.replace("{id}", &w.vault.0).replace("{page}", "0");
        let path = path.split('{').enumerate().fold(String::new(), |out, (i, part)| {
            if i == 0 {
                part.to_string()
            } else {
                format!("{out}x{}", part.split_once('}').map_or("", |(_, rest)| rest))
            }
        });
        for method in item.as_object().unwrap().keys() {
            let upper = method.to_uppercase();
            if public.contains(&template.as_str()) {
                continue;
            }
            let anonymous = Request::builder().method(upper.as_str()).uri(&path).body(Body::empty()).unwrap();
            let (anon, body, _) = w.send(anonymous).await;
            // A route that validates its input before authenticating says 400 either way, and it still can't succeed.
            assert!(
                matches!(anon, StatusCode::UNAUTHORIZED | StatusCode::BAD_REQUEST),
                "{upper} {template} with nothing: {anon} {}",
                String::from_utf8_lossy(&body)
            );
            refused += usize::from(anon == StatusCode::UNAUTHORIZED);
            // Deleting the vault would change what the later routes see.
            if upper == "DELETE" && template == "/api/v1/vaults/{id}" {
                continue;
            }
            let status = w.with_cookie(&upper, &path, &cookie).await;
            assert_ne!(status, StatusCode::UNAUTHORIZED, "{upper} {template} with a cookie");
            assert_ne!(status, StatusCode::FORBIDDEN, "{upper} {template} with an admin's cookie");
            checked += 1;
        }
    }
    assert!(checked > 40, "only {checked} routes checked");
    assert!(refused > 35, "only {refused} routes refused with 401");
}

#[tokio::test]
async fn a_state_changing_cookie_request_without_the_header_is_refused_and_changes_nothing() {
    let w = world();
    let cookie = w.cookie(&w.admin).await;
    let before = w.s.registry().fingerprint();
    for (method, path) in [
        ("POST", "/api/v1/vaults".to_string()),
        ("PATCH", format!("/api/v1/vaults/{}", w.vault.0)),
        ("DELETE", format!("/api/v1/vaults/{}", w.vault.0)),
        ("DELETE", "/api/v1/session".to_string()),
    ] {
        let req = Request::builder()
            .method(method)
            .uri(&path)
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .header("origin", "https://evil.example")
            .body(Body::from(json!({ "name": "hijacked" }).to_string()))
            .unwrap();
        let (status, body, _) = w.send(req).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
        assert_eq!(code(&body), "csrf");
    }
    assert_eq!(w.s.registry().fingerprint(), before);
    // Reads don't need it, so <img> and EventSource work.
    assert_eq!(
        w.send(Request::builder().uri("/api/v1/vaults").header("cookie", &cookie).body(Body::empty()).unwrap())
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn signing_in_needs_the_header_so_another_site_can_not_sign_a_browser_in() {
    let w = world();
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/session")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "token": w.admin }).to_string()))
        .unwrap();
    let (status, body, headers) = w.send(req).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(code(&body), "csrf");
    assert!(!headers.contains_key("set-cookie"));
}

#[tokio::test]
async fn bearer_requests_need_no_csrf_header() {
    let w = world();
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/vaults")
        .header("authorization", format!("Bearer {}", w.admin))
        .header("content-type", "application/json")
        .body(Body::from(json!({ "name": "by-bearer" }).to_string()))
        .unwrap();
    assert_eq!(w.send(req).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn a_bad_bearer_header_does_not_fall_back_to_a_good_cookie() {
    let w = world();
    let cookie = w.cookie(&w.admin).await;
    let req = Request::builder()
        .uri("/api/v1/whoami")
        .header("cookie", &cookie)
        .header("authorization", "Bearer mvx_wrong")
        .body(Body::empty())
        .unwrap();
    assert_eq!(w.send(req).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn revoking_a_token_ends_its_sessions_at_once_and_no_others() {
    let w = world();
    let c = w.s.authenticate(&w.admin).unwrap();
    let p = w.s.create_principal(&c, "someone", PrincipalKind::Human, false).unwrap();
    let new = || NewToken {
        principal_id: Some(p.id.clone()),
        name: "t".into(),
        grants: vec![],
        expires_at_ms: None,
    };
    let (one, other) = (w.s.create_token(&c, new()).unwrap(), w.s.create_token(&c, new()).unwrap());
    let (first, second, third) = (w.cookie(&one.token).await, w.cookie(&one.token).await, w.cookie(&other.token).await);
    assert_ne!(first, second, "each sign-in gets its own id");
    assert_eq!(w.with_cookie("GET", "/api/v1/whoami", &first).await, StatusCode::OK);
    w.s.revoke_token(&c, &one.row.id).unwrap();
    assert_eq!(w.with_cookie("GET", "/api/v1/whoami", &first).await, StatusCode::UNAUTHORIZED);
    assert_eq!(w.with_cookie("GET", "/api/v1/whoami", &second).await, StatusCode::UNAUTHORIZED);
    assert_eq!(w.with_cookie("GET", "/api/v1/whoami", &third).await, StatusCode::OK);
}

#[tokio::test]
async fn disabling_a_principal_or_expiring_the_token_ends_sessions_too() {
    let w = world();
    let c = w.s.authenticate(&w.admin).unwrap();
    let p = w.s.create_principal(&c, "someone", PrincipalKind::Human, false).unwrap();
    let new = |expires_at_ms| NewToken {
        principal_id: Some(p.id.clone()),
        name: "t".into(),
        grants: vec![],
        expires_at_ms,
    };
    let soon = w.s.create_token(&c, new(Some(1_700_000_001_000))).unwrap();
    let forever = w.s.create_token(&c, new(None)).unwrap();
    let (short, long) = (w.cookie(&soon.token).await, w.cookie(&forever.token).await);
    w.s.engine().clock().advance(Duration::from_secs(2));
    assert_eq!(w.with_cookie("GET", "/api/v1/whoami", &short).await, StatusCode::UNAUTHORIZED);
    assert_eq!(w.with_cookie("GET", "/api/v1/whoami", &long).await, StatusCode::OK);
    w.s.disable_principal(&c, &p.id).unwrap();
    assert_eq!(w.with_cookie("GET", "/api/v1/whoami", &long).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_session_expires_by_the_clock_and_a_later_sign_in_sweeps_it() {
    let w = world();
    let cookie = w.cookie(&w.admin).await;
    let id = cookie.split_once('=').unwrap().1.to_string();
    w.s.engine().clock().advance(Duration::from_millis(SESSION_TTL_MS as u64 - 1));
    assert_eq!(w.with_cookie("GET", "/api/v1/whoami", &cookie).await, StatusCode::OK);
    w.s.engine().clock().advance(Duration::from_millis(1));
    assert_eq!(w.with_cookie("GET", "/api/v1/whoami", &cookie).await, StatusCode::UNAUTHORIZED);
    let with_row = w.s.registry().fingerprint();
    assert!(with_row.contains("SessionRow"));
    w.cookie(&w.admin).await;
    assert_eq!(w.s.registry().fingerprint().matches("SessionRow").count(), 1, "only the new one is left");
    assert!(w.s.authenticate_session(&id).is_err());
}

#[tokio::test]
async fn tampered_and_foreign_cookies_are_refused_like_any_other_bad_credential() {
    let w = world();
    let cookie = w.cookie(&w.admin).await;
    let mut tampered = cookie.clone();
    let last = if tampered.ends_with('0') { '1' } else { '0' };
    tampered.pop();
    tampered.push(last);
    let bodies = [
        tampered,
        format!("{}x", cookie),
        format!("{SESSION_COOKIE}="),
        format!("{SESSION_COOKIE}=mxs_"),
        format!("{SESSION_COOKIE}={}", "a".repeat(5000)),
        // A token is not a session id, in either direction.
        format!("{SESSION_COOKIE}={}", w.admin),
        "memex_session_other=abc".to_string(),
    ];
    for bad in bodies {
        let req = Request::builder().uri("/api/v1/whoami").header("cookie", &bad).body(Body::empty()).unwrap();
        let (status, body, _) = w.send(req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{bad}");
        assert_eq!(code(&body), "unauthenticated");
    }
    let id = cookie.split_once('=').unwrap().1;
    let req = Request::builder()
        .uri("/api/v1/whoami")
        .header("authorization", format!("Bearer {id}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(w.send(req).await.0, StatusCode::UNAUTHORIZED);
    // Other cookies beside it are fine.
    let both = format!("theme=dark; {cookie}; other=1");
    assert_eq!(w.with_cookie("GET", "/api/v1/whoami", &both).await, StatusCode::OK);
}

#[tokio::test]
async fn logging_out_ends_the_session_clears_the_cookie_and_can_repeat() {
    let w = world();
    let cookie = w.cookie(&w.admin).await;
    let req = || {
        Request::builder()
            .method("DELETE")
            .uri("/api/v1/session")
            .header("cookie", &cookie)
            .header(CSRF_HEADER, "1")
            .body(Body::empty())
            .unwrap()
    };
    let (status, _, headers) = w.send(req()).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(headers["set-cookie"].to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(w.s.registry().audit_log(1).unwrap()[0].action, "session.end");
    assert_eq!(w.with_cookie("GET", "/api/v1/whoami", &cookie).await, StatusCode::UNAUTHORIZED);
    assert_eq!(w.send(req()).await.0, StatusCode::NO_CONTENT);
    // The token itself still works.
    assert!(w.s.authenticate(&w.admin).is_ok());
}

#[tokio::test]
async fn wrong_tokens_are_rate_limited_even_when_the_right_one_follows() {
    let w = world();
    for _ in 0..LOGIN_FAILURE_LIMIT {
        let (status, headers) = w.login("mvx_wrong").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(!headers.contains_key("set-cookie"));
    }
    let (status, body, headers) = w.send(login_request(&w.admin)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(code(&body), "rate_limited");
    assert_eq!(headers["retry-after"], "60");
    assert!(!headers.contains_key("set-cookie"));
    // The API's bearer route isn't affected, and the wait ends.
    assert!(w.s.authenticate(&w.admin).is_ok());
    w.s.engine().clock().advance(Duration::from_millis(LOGIN_WINDOW_MS as u64));
    assert_eq!(w.login(&w.admin).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn a_right_token_does_not_count_against_the_limit() {
    let w = world();
    for _ in 0..LOGIN_FAILURE_LIMIT + 5 {
        assert_eq!(w.login(&w.admin).await.0, StatusCode::CREATED);
    }
}

#[tokio::test]
async fn security_headers_are_on_every_kind_of_response() {
    let w = world();
    let cookie = w.cookie(&w.admin).await;
    let cases = [
        get("/api/v1/health"),
        get("/api/v1/whoami"),
        get("/api/v1/no-such-route"),
        get("/"),
        get("/assets/index-abc123.js"),
        Request::builder().uri("/api/v1/vaults").header("cookie", &cookie).body(Body::empty()).unwrap(),
        Request::builder()
            .method("POST")
            .uri("/api/v1/vaults")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
        Request::builder().method("PUT").uri("/api/v1/whoami").body(Body::empty()).unwrap(),
    ];
    for req in cases {
        let uri = req.uri().clone();
        let (_, _, h) = w.send(req).await;
        let csp = h["content-security-policy"].to_str().unwrap();
        assert!(csp.contains("default-src 'self'") && csp.contains("frame-ancestors 'none'"), "{uri}: {csp}");
        assert_eq!(h["x-content-type-options"], "nosniff", "{uri}");
        assert_eq!(h["x-frame-options"], "DENY", "{uri}");
        assert_eq!(h["referrer-policy"], "no-referrer", "{uri}");
    }
    assert!(!w.send(get("/")).await.2.contains_key("access-control-allow-origin"), "no CORS in production");
}

#[tokio::test]
async fn an_uploaded_html_file_downloads_as_an_attachment_in_a_sandbox() {
    let w = world();
    let html = b"<!doctype html><script>localStorage.setItem('pwned', '1')</script>".to_vec();
    let bearer = format!("Bearer {}", w.admin);
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/vaults/{}/sources?name=evil.html", w.vault.0))
        .header("authorization", &bearer)
        .body(Body::from(html.clone()))
        .unwrap();
    let (status, body, _) = w.send(req).await;
    assert_eq!(status, StatusCode::CREATED, "{}", String::from_utf8_lossy(&body));
    let id = serde_json::from_slice::<Value>(&body).unwrap()["source"]["id"].as_str().unwrap().to_string();
    let cookie = w.cookie(&w.admin).await;
    let req = Request::builder()
        .uri(format!("/api/v1/vaults/{}/sources/{id}/file", w.vault.0))
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let (status, bytes, h) = w.send(req).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, html);
    assert_eq!(h["content-disposition"], "attachment; filename=\"evil.html\"");
    assert_eq!(h["content-type"], "application/octet-stream");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert!(h["content-security-policy"].to_str().unwrap().contains("sandbox"));
}

#[tokio::test]
async fn fingerprinted_assets_cache_for_long_and_index_html_not_at_all() {
    let w = world();
    let (status, body, h) = w.send(get("/assets/index-abc123.js")).await;
    assert_eq!((status, body.as_slice()), (StatusCode::OK, SCRIPT));
    assert_eq!(h["cache-control"], "public, max-age=31536000, immutable");
    assert_eq!(h["content-type"], "text/javascript; charset=utf-8");
    for path in ["/", "/index.html"] {
        let (status, body, h) = w.send(get(path)).await;
        assert_eq!((status, body.as_slice()), (StatusCode::OK, INDEX), "{path}");
        assert_eq!(h["cache-control"], "no-store", "{path}");
        assert_eq!(h["content-type"], "text/html; charset=utf-8", "{path}");
    }
    // A file that isn't fingerprinted has to revalidate.
    assert_eq!(w.send(get("/robots.txt")).await.2["cache-control"], "no-cache");
}

#[tokio::test]
async fn unknown_paths_get_the_console_unless_they_are_api_paths() {
    let w = world();
    for path in ["/review", "/vaults/abc/search?q=1", "/deep/link/page.with.dots", "/apifoo"] {
        let (status, body, h) = w.send(get(path)).await;
        assert_eq!((status, body.as_slice()), (StatusCode::OK, INDEX), "{path}");
        assert_eq!(h["cache-control"], "no-store", "{path}");
    }
    for path in ["/api", "/api/", "/api/v1", "/api/v1/nope", "/api/v2/whoami"] {
        let (status, body, h) = w.send(get(path)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(code(&body), "not_found", "{path}");
        assert_eq!(h["content-type"], "application/json", "{path}");
    }
    let post = Request::builder().method("POST").uri("/review").body(Body::empty()).unwrap();
    assert_eq!(w.send(post).await.0, StatusCode::NOT_FOUND);
}

/// Each of these would reach a real file if a lookup ever touched the disk. The real table, not the fake, so this covers the embedded lookup.
#[tokio::test]
async fn path_traversal_finds_nothing_but_the_console() {
    let w = world();
    let real = router_with_console(w.s.clone(), memex_http::embedded_asset);
    let (index_status, index_body, _) = send(real.clone(), get("/no-such-page")).await;
    for path in [
        "/../Cargo.toml",
        "/../../../../Cargo.toml",
        "/assets/../../../../../Cargo.toml",
        "/%2e%2e/%2e%2e/%2e%2e/Cargo.toml",
        "/..%2f..%2f..%2fCargo.toml",
        "/assets/..%2f..%2f..%2f..%2f..%2fCargo.toml",
        "/....//....//Cargo.toml",
        "//etc/passwd",
        "/assets/%2e%2e/index.html",
    ] {
        let (status, body, _) = send(real.clone(), get(path)).await;
        assert_eq!(status, index_status, "{path}");
        assert_eq!(body, index_body, "{path}");
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains("[package]") && !text.contains("root:"), "{path} leaked: {text}");
    }
    assert!(memex_http::embedded_asset("../../Cargo.toml").is_none());
    assert!(memex_http::embedded_asset("/etc/passwd").is_none());
}

#[tokio::test]
async fn a_build_without_a_console_says_so_and_the_api_still_works() {
    let w = world();
    let app = router_with_console(w.s.clone(), no_console);
    for path in ["/", "/index.html", "/review", "/assets/index-abc123.js"] {
        let (status, body, h) = send(app.clone(), get(path)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert!(String::from_utf8_lossy(&body).contains("pnpm build"), "{path}");
        assert_eq!(h["content-type"], "text/html; charset=utf-8", "{path}");
        assert_eq!(h["cache-control"], "no-store", "{path}");
    }
    assert_eq!(send(app, get("/api/v1/health")).await.0, StatusCode::OK);
}

/// Spec 046: an agent signed in to a console session decides nothing, whatever its scope. Its token, used as a token, keeps what spec 025 recorded.
#[tokio::test]
async fn an_admin_agent_decides_nothing_through_a_session_and_keeps_what_its_bearer_token_may_do() {
    let w = world();
    let root = w.s.authenticate(&w.admin).unwrap();
    let p = w.s.create_principal(&root, "agent", PrincipalKind::Agent, false).unwrap();
    let token = w
        .s
        .create_token(
            &root,
            NewToken {
                principal_id: Some(p.id),
                name: "t".into(),
                grants: vec![memex_types::Grant {
                    vault: w.vault.clone(),
                    scope: memex_types::Scope::Admin,
                }],
                expires_at_ms: None,
            },
        )
        .unwrap()
        .token;
    let base = format!("/api/v1/vaults/{}", w.vault.0);
    let bearer = |path: String, token: &str, body: Value| {
        Request::builder()
            .method("POST")
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let propose = async |key: &str| -> String {
        let body = json!({"ops": [{"op": "create_page", "id": format!("page-{key}"), "title": format!("Page {key}"), "body": "text"}], "rationale": "why", "idempotency_key": key});
        let (status, out, _) = w.send(bearer(format!("{base}/proposals"), &token, body)).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&out));
        let out: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(out["changeset"]["state"], "pending", "an agent's page waits for review");
        out["changeset"]["id"].as_str().unwrap().to_string()
    };
    let cookie = w.cookie(&token).await;
    let session = |path: String, body: Value| {
        Request::builder()
            .method("POST")
            .uri(path)
            .header("cookie", cookie.clone())
            .header(CSRF_HEADER, "1")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let cs = propose("one").await;
    let edit = json!({"ops": [{"op": "create_page", "id": "page-one", "title": "Edited", "body": "text"}], "reason": "r"});
    let attempts = [
        (format!("{base}/changesets/{cs}/approve"), json!({"reason": "r"})),
        (format!("{base}/changesets/{cs}/reject"), json!({"reason": "r"})),
        (format!("{base}/review/items/{cs}/edit-approve"), edit),
        (format!("{base}/review/batch"), json!({"decisions": [{"id": cs, "decision": "approve"}]})),
        (format!("{base}/changesets/{cs}/revert"), json!({"reason": "r"})),
    ];
    for (path, body) in attempts {
        let (status, out, _) = w.send(session(path.clone(), body)).await;
        assert_eq!((status, code(&out).as_str()), (StatusCode::FORBIDDEN, "agent_session"), "{path}");
    }
    let (status, out, _) = w.send(get_with_cookie(&format!("{base}/changesets/{cs}"), &cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(serde_json::from_slice::<Value>(&out).unwrap()["state"], "pending", "none of that decided anything");

    // The same agent with its token: an ordinary item can be approved and reverted, as spec 025 recorded. This is the gap the session rule leaves.
    let (status, out, _) = w.send(bearer(format!("{base}/changesets/{cs}/approve"), &token, json!({"reason": "r"}))).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&out));
    assert_eq!(serde_json::from_slice::<Value>(&out).unwrap()["state"], "committed");
    let (status, _, _) = w.send(bearer(format!("{base}/changesets/{cs}/revert"), &token, json!({"reason": "r"}))).await;
    assert_eq!(status, StatusCode::OK);

    // A person's session is untouched.
    let second = propose("two").await;
    let person = w.cookie(&w.admin).await;
    let req = Request::builder()
        .method("POST")
        .uri(format!("{base}/changesets/{second}/approve"))
        .header("cookie", person)
        .header(CSRF_HEADER, "1")
        .header("content-type", "application/json")
        .body(Body::from(json!({"reason": "r"}).to_string()))
        .unwrap();
    let (status, _, _) = w.send(req).await;
    assert_eq!(status, StatusCode::OK);

    let doc = serde_json::to_string(&openapi()).unwrap();
    assert!(doc.contains("agent_session"), "the contract names the code");
}

fn get_with_cookie(path: &str, cookie: &str) -> Request<Body> {
    Request::builder().uri(path).header("cookie", cookie).body(Body::empty()).unwrap()
}
