//! No token may reach the logs. A test binary of its own, because it installs the global log subscriber.
use axum::body::Body;
use axum::http::Request;
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
use memex_core::{Server, ServerEdges};
use memex_http::router;
use std::io::Write;
use std::sync::{Arc, Mutex};
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
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn tokens_do_not_appear_in_logs() {
    let capture = Capture::default();
    let sink = capture.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || sink.clone())
        .init();
    let s: Arc<Server<Fakes>> = Arc::new(Server::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(5),
        SeededRandom::new(3),
        memex_core::testing::MemLexical::new(),
    ));
    let token = s.bootstrap("root").unwrap().token;
    for (method, path) in [
        ("GET", "/api/v1/whoami".to_string()),
        ("GET", format!("/api/v1/vaults?cursor={token}")),
        ("GET", "/nope".to_string()),
    ] {
        let req = Request::builder()
            .method(method)
            .uri(&path)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        router(s.clone()).oneshot(req).await.unwrap();
    }
    let wrong = Request::builder()
        .uri("/api/v1/whoami")
        .header("authorization", "Bearer mvx_a_wrong_secret")
        .body(Body::empty())
        .unwrap();
    router(s.clone()).oneshot(wrong).await.unwrap();
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("request"), "logging must really be on: {logs}");
    assert!(!logs.contains(&token), "{logs}");
    assert!(!logs.contains("mvx_"), "{logs}");
}
