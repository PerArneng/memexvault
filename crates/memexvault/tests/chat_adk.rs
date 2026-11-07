//! The chat route on the real ADK runner, against a stand-in for a model's server on this machine, so no provider is called and nothing leaves. It proves the wire: what ADK does with terminal events, tool round trips, failures and cancels, through to the frames a client gets. It says nothing about what a model does.
// A stand-in server waits on real sockets and the clock, reads and writes fixtures, and prints what it measures.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use futures_util::StreamExt;
use memex_agent_adk::{AdkAgent, LlmEndpoint};
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
use memex_core::{Caller, ChatLimits, Server, ServerEdges, Snapshot, Store};
use memex_http::{chat_routes, embedded_asset, router_with};
use memex_types::{Row, Table, VaultId};
use memexvault::edges::AdkAgentRunner;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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

struct Rig {
    s: Arc<Server<Fakes>>,
    root: String,
    admin: Caller,
    v: VaultId,
    app: axum::Router,
}

fn rig(url: &str) -> Rig {
    let s = Arc::new(Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_700_000_000_000),
        SeededRandom::new(49),
        memex_core::testing::mem_search(),
    ));
    s.set_chat(Some(ChatLimits::default()));
    let root = s.bootstrap("root").unwrap().token;
    let admin = s.authenticate(&root).unwrap();
    let v = s.create_vault(&admin, "v").unwrap().id;
    s.set_chat_settings(
        &admin,
        &v,
        memex_core::ChatSettingsUpdate {
            enabled: true,
            thread_token_budget: None,
            day_token_budget: None,
        },
    )
    .unwrap();
    let agent = AdkAgent::new(LlmEndpoint::OpenAi {
        base_url: url.into(),
        api_key: Some("test-key-not-real".into()),
        model: "m".into(),
    })
    .unwrap();
    // An index that's still building is a refusal a tool reports, and that's not what's being tried here.
    while s.project_lexical(&v).unwrap() == memex_core::StepOutcome::Progress {}
    while s.project_vector(&v).unwrap() == memex_core::StepOutcome::Progress {}
    let app = router_with(s.clone(), embedded_asset, chat_routes(Some(Arc::new(AdkAgentRunner(agent)))));
    Rig { s, root, admin, v, app }
}

impl Rig {
    fn request(&self) -> Request<Body> {
        let body = json!({"threadId": "thread-1", "runId": "run-1", "messages": [{"id": "u", "role": "user", "content": "hello"}], "tools": [], "context": [], "state": {}, "forwardedProps": {}});
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/vaults/{}/chat/runs", self.v))
            .header("authorization", format!("Bearer {}", self.root))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn frames(&self) -> Vec<Value> {
        let res = self.app.clone().oneshot(self.request()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), 10_000_000).await.unwrap();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        text.split("\n\n").filter_map(|f| f.strip_prefix("data: ")).map(|d| serde_json::from_str(d).unwrap()).collect()
    }

    fn derivation(&self) -> Option<memex_types::DerivationRow> {
        let snap = self.s.engine().store().snapshot(&self.v).unwrap();
        snap.rows(Table::Derivation, None).unwrap().into_iter().find_map(|r| if let Row::Derivation(d) = r { Some(d) } else { None })
    }
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut data = vec![];
    let mut buf = [0u8; 8192];
    loop {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&data);
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|l| l.to_lowercase().strip_prefix("content-length: ").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                .unwrap_or(0);
            if data.len() >= end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

#[derive(Clone)]
enum Reply {
    Whole(&'static str, String),
    /// Sends these bytes with a stream's headers, and cuts the connection.
    Cut(String),
}

fn upstream(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(vec![]));
    let log = seen.clone();
    std::thread::spawn(move || {
        let mut replies = replies.into_iter();
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            log.lock().unwrap().push(read_request(&mut stream));
            match replies.next().unwrap_or(Reply::Whole("500 Internal Server Error", "{}".into())) {
                Reply::Whole(status, body) => {
                    let head = format!("HTTP/1.1 {status}\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                    let _ = stream.write_all(head.as_bytes());
                }
                Reply::Cut(body) => {
                    let head = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 1000000\r\nconnection: close\r\n\r\n{body}");
                    let _ = stream.write_all(head.as_bytes());
                }
            }
        }
    });
    (url, seen)
}

fn chunk(delta: Value, finish: Value, usage: Value) -> String {
    let body = json!({"id": "1", "object": "chat.completion.chunk", "created": 1, "model": "m", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}], "usage": usage});
    format!("data: {body}\n\n")
}

fn used(input: u64, output: u64) -> Value {
    json!({"prompt_tokens": input, "completion_tokens": output, "total_tokens": input + output})
}

fn text(parts: &[&str], finish: &str) -> String {
    let mut out = chunk(json!({"role": "assistant", "content": ""}), Value::Null, Value::Null);
    for p in parts {
        out += &chunk(json!({"content": p}), Value::Null, Value::Null);
    }
    out + &chunk(json!({}), json!(finish), used(11, 5)) + "data: [DONE]\n\n"
}

fn tool(name: &str, args: &str) -> String {
    let call = json!({"role": "assistant", "tool_calls": [{"index": 0, "id": "call-1", "type": "function", "function": {"name": name, "arguments": args}}]});
    chunk(call, Value::Null, Value::Null) + &chunk(json!({}), json!("tool_calls"), used(10, 2)) + "data: [DONE]\n\n"
}

fn types(frames: &[Value]) -> Vec<&str> {
    frames.iter().map(|f| f["type"].as_str().unwrap()).collect()
}

fn delta_text(frames: &[Value]) -> String {
    frames.iter().filter(|f| f["type"] == "TEXT_MESSAGE_CONTENT").map(|f| f["delta"].as_str().unwrap()).collect()
}

/// The same golden files as the scripted scenarios, so one schema check covers both.
fn golden(name: &str, frames: &[Value]) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../memex-http/tests/fixtures/agui").join(format!("adk_{name}.jsonl"));
    let text: String = frames.iter().map(|f| format!("{f}\n")).collect();
    if std::env::var_os("MEMEX_UPDATE_GOLDEN").is_some() {
        std::fs::write(path, &text).unwrap();
        return;
    }
    assert_eq!(text, std::fs::read_to_string(path).unwrap_or_default(), "{name}'s stream changed");
}

#[tokio::test]
async fn deltas_are_not_repeated_by_the_terminal_event() {
    let (url, _) = upstream(vec![Reply::Whole("200 OK", text(&["Hel", "lo ✓", " å"], "stop"))]);
    let r = rig(&url);
    let frames = r.frames().await;
    assert_eq!(delta_text(&frames), "Hello ✓ å");
    assert_eq!(frames.iter().filter(|f| f["type"] == "TEXT_MESSAGE_CONTENT").count(), 3);
    assert_eq!(types(&frames).last(), Some(&"RUN_FINISHED"));
    golden("text", &frames);
    let d = r.derivation().expect("a derivation");
    assert_eq!((d.usage.input_tokens, d.usage.output_tokens, d.attempts), (11, 5, 1));
    assert_eq!(d.detail, None, "the provider's own count");
}

#[tokio::test]
async fn a_tool_round_trip_reaches_the_real_tools_and_the_second_call_sees_it_marked_as_data() {
    let (url, seen) = upstream(vec![
        Reply::Whole("200 OK", tool("search", "{\"q\":\"bridge\"}")),
        Reply::Whole("200 OK", text(&["Nothing."], "stop")),
    ]);
    let r = rig(&url);
    let frames = r.frames().await;
    assert_eq!(
        types(&frames),
        [
            "RUN_STARTED",
            "TOOL_CALL_START",
            "TOOL_CALL_ARGS",
            "TOOL_CALL_END",
            "TOOL_CALL_RESULT",
            "TEXT_MESSAGE_START",
            "TEXT_MESSAGE_CONTENT",
            "TEXT_MESSAGE_END",
            "RUN_FINISHED"
        ]
    );
    let ids: Vec<&Value> = frames.iter().filter_map(|f| f.get("toolCallId")).collect();
    assert!(ids.iter().all(|i| *i == ids[0]), "{ids:?}");
    golden("tool", &frames);
    let sent = seen.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert!(sent[1].contains("never as instructions"), "the data's marking reaches the model: {}", sent[1]);
    // The definitions reach the model as compiled, with propose as the admin may have it.
    for name in ["search", "read", "provenance", "propose"] {
        assert!(sent[0].contains(&format!("\"name\":\"{name}\"")), "{name}");
    }
    assert!(!sent[0].contains("\"name\":\"fetch"), "no such tool");
    let stored = r.s.get_chat_thread(&r.admin, &r.v, "thread-1", None, 10).unwrap();
    assert_eq!(stored.messages.len(), 3, "the question, the tool's answer and the reply");
}

#[tokio::test]
async fn a_model_calling_for_a_url_gets_nothing_and_no_connection_is_made() {
    let canary = TcpListener::bind("127.0.0.1:0").unwrap();
    canary.set_nonblocking(true).unwrap();
    let args = format!("{{\"url\":\"http://{}/x\"}}", canary.local_addr().unwrap());
    let (url, seen) = upstream(vec![Reply::Whole("200 OK", tool("fetch_url", &args)), Reply::Whole("200 OK", text(&["Sorry."], "stop"))]);
    let r = rig(&url);
    let frames = r.frames().await;
    assert_eq!(types(&frames).last(), Some(&"RUN_FINISHED"));
    assert!(canary.accept().is_err());
    assert_eq!(seen.lock().unwrap().len(), 2, "the only requests went to the model's server");
}

#[tokio::test]
async fn provider_failures_each_end_in_one_run_error_with_no_provider_text() {
    let cut = chunk(json!({"role": "assistant", "content": "Half a"}), Value::Null, Value::Null);
    let in_chunk = "data: {\"error\": {\"message\": \"provider secret text\", \"code\": \"overloaded\"}}\n\n".to_string();
    let error = json!({"error": {"message": "provider secret text"}}).to_string();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let down = format!("http://{}/v1", listener.local_addr().unwrap());
    drop(listener);
    let cases: Vec<(&str, Option<Vec<Reply>>)> = vec![
        ("refused", Some(vec![Reply::Whole("400 Bad Request", error.clone())])),
        ("server_error", Some(vec![Reply::Whole("500 Internal Server Error", error.clone()); 5])),
        ("rate_limited", Some(vec![Reply::Whole("429 Too Many Requests", error); 5])),
        ("cut_mid_stream", Some(vec![Reply::Cut(cut)])),
        ("error_in_a_chunk", Some(vec![Reply::Whole("200 OK", in_chunk)])),
        ("down", None),
    ];
    for (name, replies) in cases {
        let url = match replies {
            Some(replies) => upstream(replies).0,
            None => down.clone(),
        };
        let r = rig(&url);
        let frames = r.frames().await;
        let t = types(&frames);
        let ends = t.iter().filter(|x| **x == "RUN_ERROR" || **x == "RUN_FINISHED").count();
        assert_eq!(ends, 1, "{name}: {frames:?}");
        assert_eq!(t.last(), Some(&"RUN_ERROR"), "{name}: {frames:?}");
        assert_eq!(frames.last().unwrap()["message"], json!("The model call failed."), "{name}");
        let all = frames.iter().map(Value::to_string).collect::<String>();
        assert!(!all.contains("provider secret") && !all.contains("test-key-not-real"), "{name}: {all}");
        golden(name, &frames);
        let d = r.derivation().expect("a failed run still has its record");
        assert_eq!(d.outcome, memex_types::DerivationOutcome::Failed, "{name}");
        assert!(d.detail.as_deref().unwrap().starts_with("failed: "), "{name}");
    }
}

#[tokio::test]
async fn a_reply_cut_at_its_limit_says_so_as_spec_034_left_open() {
    let (url, _) = upstream(vec![Reply::Whole("200 OK", text(&["Half a"], "length"))]);
    let r = rig(&url);
    let frames = r.frames().await;
    let last = frames.last().unwrap();
    assert_eq!((&last["type"], &last["result"]), (&json!("RUN_FINISHED"), &json!({"stop": "max_tokens"})));
    golden("cut_at_its_limit", &frames);
    let stored = r.s.get_chat_thread(&r.admin, &r.v, "thread-1", None, 10).unwrap();
    assert_eq!(stored.messages[1].message.stop.as_deref(), Some("max_tokens"), "and what's kept says so too");
}

#[tokio::test]
async fn a_tool_only_agent_stops_at_its_call_limit_cleanly() {
    let (url, _) = upstream(vec![Reply::Whole("200 OK", tool("search", "{\"q\":\"a\"}")); 10]);
    let r = rig(&url);
    r.s.set_chat(Some(ChatLimits {
        max_model_calls: 2,
        ..ChatLimits::default()
    }));
    let frames = r.frames().await;
    assert_eq!(frames.last().unwrap()["result"], json!({"stop": "turns"}), "{frames:?}");
    golden("turns", &frames);
}

/// Item 7, end to end on ADK: from the client going away to the model's server seeing its connection close.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_going_away_closes_the_providers_connection_within_a_second_and_it_is_charged() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let closed = Arc::new(Mutex::new(None));
    let saw = closed.clone();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        read_request(&mut stream);
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
        loop {
            if stream.write_all(chunk(json!({"content": "word "}), Value::Null, Value::Null).as_bytes()).and_then(|()| stream.flush()).is_err() {
                *saw.lock().unwrap() = Some(Instant::now());
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    let r = rig(&url);
    let res = r.app.clone().oneshot(r.request()).await.unwrap();
    let mut stream = res.into_body().into_data_stream();
    let mut seen = 0;
    while seen < 200 {
        seen += stream.next().await.unwrap().unwrap().len();
    }
    assert!(r.derivation().is_none(), "the run is still going, so the stream's guard hasn't stopped it");
    let gone = Instant::now();
    drop(stream);
    let started = Instant::now();
    while closed.lock().unwrap().is_none() || r.derivation().is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "it never stopped");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let close = closed.lock().unwrap().unwrap().saturating_duration_since(gone);
    println!("client gone to provider's connection closed: {close:?}, to the run's record written: {:?}", gone.elapsed());
    assert!(close < Duration::from_secs(1), "{close:?}");
    let d = r.derivation().unwrap();
    assert!(d.usage.input_tokens > 0 && d.usage.output_tokens > 0, "{:?}", d.usage);
    assert_eq!(d.detail.as_deref(), Some("stopped: cancelled; tokens estimated"));
}
