//! Spike server: `POST /agent/{provider}/{mode}` takes AG-UI's RunAgentInput and answers with AG-UI events over SSE.
//! `GET /runs/{id}` reports what the server saw, for the client's checks. Env: GEMINI_API_KEY, BIND (default 127.0.0.1:18034).
mod adk;
mod mapping;
mod scripted;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{sse::{Event, KeepAlive, Sse}, IntoResponse, Response},
    routing::{get, post},
    serve::ListenerExt,
    Json, Router,
};
use futures::{Stream, StreamExt};
use mapping::{AgUi, Mapper};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, convert::Infallible, sync::{Arc, Mutex}, time::{Duration, SystemTime, UNIX_EPOCH}};
use tokio_util::sync::CancellationToken;

pub fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_micros() as f64 / 1000.0
}

/// Wall-clock milliseconds, so the client on this machine can compare them with its own.
#[derive(Default, Clone, Serialize)]
pub struct Stats {
    pub request_ms: f64,
    pub model_call_ms: Option<f64>,
    pub first_chunk_ms: Option<f64>,
    pub first_delta_out_ms: Option<f64>,
    pub disconnect_ms: Option<f64>,
    pub model_end_ms: Option<f64>,
    pub model_end: String,
    pub tool_started_ms: Option<f64>,
    pub tool_dropped_ms: Option<f64>,
    pub chunks: u64,
    pub chunks_after_disconnect: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub finals: Vec<(String, String)>,
    pub final_text: String,
    pub error_detail: Option<String>,
}

impl Stats {
    pub fn record_final(&mut self, turn: &str, full: &str) {
        match self.finals.iter_mut().find(|(t, _)| t == turn) {
            Some(entry) => entry.1 = full.to_string(),
            None => self.finals.push((turn.to_string(), full.to_string())),
        }
        self.final_text = self.finals.iter().map(|(_, t)| t.as_str()).collect();
    }
}

#[derive(Clone)]
struct App {
    runs: Arc<Mutex<HashMap<String, Arc<Mutex<Stats>>>>>,
    gemini: Option<Arc<dyn adk_core::Llm>>,
    local: Arc<dyn adk_core::Llm>,
    fake: Arc<dyn adk_core::Llm>,
    fake_cut: Arc<dyn adk_core::Llm>,
    fake_slow: Arc<dyn adk_core::Llm>,
    fake_500: Arc<dyn adk_core::Llm>,
    down: Arc<dyn adk_core::Llm>,
    bad_key: Arc<dyn adk_core::Llm>,
    key: String,
}

// Fields checked against @ag-ui/core's RunAgentInputSchema. Others, such as tools, context and state, are ignored here.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunAgentInput {
    thread_id: String,
    run_id: String,
    messages: Vec<Message>,
    #[serde(default)]
    forwarded_props: serde_json::Value,
}

#[derive(Deserialize)]
struct Message {
    role: String,
    #[serde(default)]
    content: serde_json::Value,
}

/// Dropped with the response stream, which is how a client's disconnect arrives.
struct DisconnectGuard { token: CancellationToken, stats: Arc<Mutex<Stats>>, finished: bool }
impl Drop for DisconnectGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.stats.lock().unwrap().disconnect_ms = Some(now_ms());
            self.token.cancel();
        }
    }
}

fn frame(e: &AgUi, stats: &Mutex<Stats>) -> Result<Event, Infallible> {
    if matches!(e, AgUi::TextMessageContent { .. }) { stats.lock().unwrap().first_delta_out_ms.get_or_insert_with(now_ms); }
    Ok(Event::default().data(serde_json::to_string(e).expect("serializes")))
}

fn run_stream(input: RunAgentInput, req: adk::RunRequest) -> impl Stream<Item = Result<Event, Infallible>> {
    let stats = req.stats.clone();
    async_stream::stream! {
        // Made in here so the stream owns the whole guard and it drops only with the stream.
        let mut guard = DisconnectGuard { token: req.token.clone(), stats: stats.clone(), finished: false };
        let mut mapper = Mapper::new(input.thread_id, input.run_id);
        for e in mapper.start() { yield frame(&e, &stats); }
        match adk::start(req).await {
            Err(f) => {
                stats.lock().unwrap().error_detail = Some(f.detail);
                for e in mapper.on_error(&f.code, "The model call failed.") { yield frame(&e, &stats); }
            }
            Ok(mut events) => {
                let mut failed = false;
                while let Some(item) = events.next().await {
                    match item {
                        Ok(ev) => for e in mapper.on_event(&ev) { yield frame(&e, &stats); },
                        Err(f) => {
                            // The detail may hold provider text, so it stays on the server. The client gets a code.
                            stats.lock().unwrap().error_detail = Some(f.detail);
                            for e in mapper.on_error(&f.code, "The model call failed.") { yield frame(&e, &stats); }
                            failed = true;
                            break;
                        }
                    }
                }
                if !failed { for e in mapper.finish() { yield frame(&e, &stats); } }
            }
        }
        guard.finished = true;
    }
}

async fn agent(State(app): State<App>, Path((provider, mode)): Path<(String, String)>, Json(input): Json<RunAgentInput>) -> Response {
    let request_ms = now_ms();
    let stats = Arc::new(Mutex::new(Stats { request_ms, ..Default::default() }));
    app.runs.lock().unwrap().insert(input.run_id.clone(), stats.clone());
    let (llm, capped): (Arc<dyn adk_core::Llm>, bool) = match provider.as_str() {
        "scripted" => (Arc::new(scripted::ScriptedLlm { mode: mode.clone() }), false),
        "fake" => (app.fake.clone(), false),
        "fake-cut" => (app.fake_cut.clone(), false),
        "fake-slow" => (app.fake_slow.clone(), false),
        "fake-500" => (app.fake_500.clone(), false),
        "down" => (app.down.clone(), false),
        "bad-key" => (app.bad_key.clone(), false),
        "local" => (app.local.clone(), false),
        "gemini" => match &app.gemini { Some(g) => (g.clone(), true), None => return (StatusCode::SERVICE_UNAVAILABLE, "no GEMINI_API_KEY").into_response() },
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let prompt = input.messages.iter().rev().find(|m| m.role == "user").and_then(|m| m.content.as_str()).unwrap_or("").to_string();
    let max_output_tokens = input.forwarded_props.get("maxOutputTokens").and_then(|v| v.as_i64()).unwrap_or(400) as i32;
    let req = adk::RunRequest { llm, capped, prompt, mode, max_output_tokens, token: CancellationToken::new(), stats };
    Sse::new(run_stream(input, req)).keep_alive(KeepAlive::new().interval(Duration::from_secs(1))).into_response()
}

async fn run_stats(State(app): State<App>, Path(id): Path<String>) -> Response {
    let found = app.runs.lock().unwrap().get(&id).cloned();
    match found {
        Some(s) => { let mut s = s.lock().unwrap().clone(); s.error_detail = s.error_detail.map(|d| d.replace(&app.key, "***").chars().take(300).collect()); Json(s).into_response() }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn spend() -> Json<serde_json::Value> {
    let upstream_closed_ms = *scripted::UPSTREAM_CLOSED_MS.lock().unwrap();
    Json(serde_json::json!({"gemini_tokens": adk::GEMINI_TOKENS.load(std::sync::atomic::Ordering::Relaxed), "cap": adk::GEMINI_TOKEN_CAP, "upstream_closed_ms": upstream_closed_ms}))
}

#[tokio::main]
async fn main() {
    let bind = std::env::var("BIND").unwrap_or_else(|_| "127.0.0.1:18034".into());
    let key = std::env::var("GEMINI_API_KEY").unwrap_or_default();
    let gemini_model = std::env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-3.5-flash-lite".into());
    let fake = |b: &str| adk::openai_compatible(&format!("http://{bind}/fake/{b}/v1"), "fake").expect("client");
    let app = App {
        runs: Default::default(),
        gemini: if key.is_empty() { None } else { Some(adk::gemini(&key, &gemini_model).expect("gemini")) },
        local: adk::openai_compatible("http://vllm.brain.home.arpa/v1", "qwen3.6-35b").expect("client"),
        fake: fake("ok"),
        fake_cut: fake("cut"),
        fake_slow: fake("slow"),
        fake_500: fake("500"),
        down: adk::openai_compatible("http://127.0.0.1:9/v1", "none").expect("client"),
        bad_key: adk::gemini("not-a-real-key", &gemini_model).expect("gemini"),
        key,
    };
    let router = Router::new()
        .route("/agent/{provider}/{mode}", post(agent))
        .route("/runs/{id}", get(run_stats))
        .route("/spend", get(spend))
        .route("/fake/{behavior}/v1/chat/completions", post(scripted::fake_upstream))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(&bind).await.expect("bind");
    // Without nodelay, small SSE frames can wait on the previous frame's ACK.
    let listener = listener.tap_io(|s| { let _ = s.set_nodelay(true); });
    eprintln!("listening on {bind}");
    axum::serve(listener, router).await.expect("serve");
}

#[cfg(test)]
mod tests {
    use super::*;
    use mapping::to_jsonl;

    /// The real ADK runner on the scripted model, so the golden sequence covers what ADK really emits, terminal repeats included.
    async fn through_adk(mode: &str) -> Vec<AgUi> {
        let req = adk::RunRequest {
            llm: Arc::new(scripted::ScriptedLlm { mode: mode.into() }), capped: false, prompt: "Hi".into(), mode: mode.into(),
            max_output_tokens: 100, token: CancellationToken::new(), stats: Default::default(),
        };
        let mut m = Mapper::new("thread", "run");
        let mut out = m.start();
        let mut events = adk::start(req).await.expect("starts");
        while let Some(item) = events.next().await {
            match item {
                Ok(ev) => out.extend(m.on_event(&ev)),
                Err(f) => { out.extend(m.on_error(&f.code, "The model call failed.")); return out; }
            }
        }
        out.extend(m.finish());
        out
    }

    fn golden(name: &str, got: &str) {
        let path = format!("{}/golden/{name}.jsonl", env!("CARGO_MANIFEST_DIR"));
        if std::env::var("UPDATE_GOLDEN").is_ok() { std::fs::write(&path, got).unwrap(); }
        assert_eq!(got, std::fs::read_to_string(&path).unwrap());
    }

    #[tokio::test]
    async fn golden_text_run() { golden("text_run", &to_jsonl(&through_adk("chat").await)); }

    #[tokio::test]
    async fn golden_tool_run() { golden("tool_run", &to_jsonl(&through_adk("tool").await)); }

    #[tokio::test]
    async fn golden_failed_run() { golden("failed_run", &to_jsonl(&through_adk("fail").await)); }
}
