//! The edge: ADK-Rust. No ADK type leaves this file, apart from the test models handed in by main.
use crate::mapping::{RunnerEvent, RunnerPart};
use crate::{now_ms, Stats};
use adk_agent::LlmAgentBuilder;
use adk_core::{Agent, Content, Event, Llm, LlmRequest, LlmResponse, LlmResponseStream, Part, RunConfig, SessionId, StreamingMode, Tool, ToolContext, UserId};
use adk_model::{gemini::GeminiModel, openai_compatible::{OpenAICompatible, OpenAICompatibleConfig}};
use adk_runner::Runner;
use adk_session::{CreateRequest, InMemorySessionService, SessionService};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use std::{pin::Pin, sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex}, task::{Context, Poll}, time::Duration};
use tokio_util::sync::CancellationToken;

pub static GEMINI_TOKENS: AtomicU64 = AtomicU64::new(0);
pub const GEMINI_TOKEN_CAP: u64 = 60_000;

pub fn to_runner_event(ev: &Event) -> RunnerEvent {
    let parts = ev.llm_response.content.iter().flat_map(|c| &c.parts).filter_map(|p| match p {
        Part::Text { text } => Some(RunnerPart::Text(text.clone())),
        Part::FunctionCall { name, args, id, .. } => Some(RunnerPart::ToolCall { id: id.clone(), name: name.clone(), args: args.clone() }),
        Part::FunctionResponse { function_response, id, .. } => Some(RunnerPart::ToolResult { id: id.clone(), name: function_response.name.clone(), result: function_response.response.clone() }),
        _ => None, // Thinking and media aren't shown in this spike.
    }).collect();
    RunnerEvent { turn: ev.id.clone(), partial: ev.llm_response.partial, parts }
}

pub fn gemini(key: &str, model: &str) -> Result<Arc<dyn Llm>, String> {
    Ok(Arc::new(GeminiModel::new(key.to_string(), model.to_string()).map_err(|e| e.to_string())?))
}

pub fn openai_compatible(base_url: &str, model: &str) -> Result<Arc<dyn Llm>, String> {
    let cfg = OpenAICompatibleConfig::new("unused", model).with_provider_name("vllm").with_base_url(base_url);
    let c = OpenAICompatible::new(cfg).map_err(|e| e.to_string())?.with_request_adapter(Arc::new(|body, _headers| {
        body["chat_template_kwargs"] = json!({"enable_thinking": false});
        Ok(())
    }));
    Ok(Arc::new(c))
}

/// Records when the model call starts, first answers and ends, and whether it was dropped before its end.
struct WatchedLlm { inner: Arc<dyn Llm>, stats: Arc<Mutex<Stats>>, capped: bool }

struct WatchedStream { inner: LlmResponseStream, stats: Arc<Mutex<Stats>>, capped: bool, ended: bool, tokens: u64 }

#[async_trait]
impl Llm for WatchedLlm {
    fn name(&self) -> &str { self.inner.name() }
    fn schema_adapter(&self) -> &dyn adk_core::SchemaAdapter { self.inner.schema_adapter() }
    fn uses_interactions_api(&self) -> bool { self.inner.uses_interactions_api() }

    async fn generate_content(&self, req: LlmRequest, stream: bool) -> adk_core::Result<LlmResponseStream> {
        if self.capped && GEMINI_TOKENS.load(Ordering::Relaxed) >= GEMINI_TOKEN_CAP {
            return Err(adk_core::AdkError::model("spike token cap reached"));
        }
        self.stats.lock().unwrap().model_call_ms.get_or_insert_with(now_ms);
        let inner = self.inner.generate_content(req, stream).await?;
        Ok(Box::pin(WatchedStream { inner, stats: self.stats.clone(), capped: self.capped, ended: false, tokens: 0 }))
    }
}

impl Stream for WatchedStream {
    type Item = adk_core::Result<LlmResponse>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let polled = self.inner.as_mut().poll_next(cx);
        match &polled {
            Poll::Ready(Some(item)) => {
                let now = now_ms();
                let mut tokens = None;
                {
                    let mut s = self.stats.lock().unwrap();
                    s.first_chunk_ms.get_or_insert(now);
                    s.chunks += 1;
                    if s.disconnect_ms.is_some() { s.chunks_after_disconnect += 1; }
                    if let Ok(r) = item {
                        if let Some(u) = &r.usage_metadata {
                            s.input_tokens = s.input_tokens.max(u.prompt_token_count.max(0) as u64);
                            s.output_tokens = s.output_tokens.max(u.candidates_token_count.max(0) as u64);
                            tokens = Some((u.prompt_token_count.max(0) + u.candidates_token_count.max(0)) as u64);
                        }
                    }
                }
                if let Some(t) = tokens { self.tokens = self.tokens.max(t); }
            }
            Poll::Ready(None) => self.ended = true,
            Poll::Pending => {}
        }
        polled
    }
}

impl Drop for WatchedStream {
    fn drop(&mut self) {
        let mut s = self.stats.lock().unwrap();
        s.model_end_ms = Some(now_ms());
        s.model_end = if self.ended { "complete" } else { "dropped" }.to_string();
        if self.capped { GEMINI_TOKENS.fetch_add(self.tokens, Ordering::Relaxed); }
    }
}

/// A made-up tool on made-up data. `delay` makes it the slow one for the cancel test.
struct FactTool { name: &'static str, delay: Duration, stats: Arc<Mutex<Stats>> }

struct ToolWatch { stats: Arc<Mutex<Stats>>, finished: bool }
impl Drop for ToolWatch {
    fn drop(&mut self) { if !self.finished { self.stats.lock().unwrap().tool_dropped_ms = Some(now_ms()); } }
}

#[async_trait]
impl Tool for FactTool {
    fn name(&self) -> &str { self.name }
    fn description(&self) -> &str { "Looks up a stored fact about a topic. Call it whenever the user asks for a fact." }
    fn parameters_schema(&self) -> Option<Value> {
        Some(json!({"type": "object", "properties": {"topic": {"type": "string"}}, "required": ["topic"]}))
    }
    async fn execute(&self, _ctx: Arc<dyn ToolContext>, _args: Value) -> adk_core::Result<Value> {
        self.stats.lock().unwrap().tool_started_ms = Some(now_ms());
        let mut watch = ToolWatch { stats: self.stats.clone(), finished: false };
        tokio::time::sleep(self.delay).await;
        watch.finished = true;
        Ok(json!({"fact": "The Zorblat Bridge is 412 metres long."}))
    }
}

pub struct RunRequest {
    pub llm: Arc<dyn Llm>,
    pub capped: bool,
    pub prompt: String,
    pub mode: String,
    pub max_output_tokens: i32,
    pub token: CancellationToken,
    pub stats: Arc<Mutex<Stats>>,
}

pub type RunnerStream = Pin<Box<dyn Stream<Item = Result<RunnerEvent, RunFailure>> + Send>>;

#[derive(Debug)]
pub struct RunFailure { pub code: String, pub detail: String }

fn failure(e: adk_core::AdkError) -> RunFailure {
    RunFailure { code: format!("{:?}", e.category).to_lowercase(), detail: e.to_string() }
}

fn identity(e: impl std::fmt::Display) -> RunFailure {
    RunFailure { code: "invalidinput".into(), detail: e.to_string() }
}

/// Starts a run. Dropping the returned stream stops it, and so does cancelling the token.
pub async fn start(req: RunRequest) -> Result<RunnerStream, RunFailure> {
    let llm: Arc<dyn Llm> = Arc::new(WatchedLlm { inner: req.llm, stats: req.stats.clone(), capped: req.capped });
    let mut b = LlmAgentBuilder::new("chat")
        .model(llm)
        .instruction("You are a test assistant. Answer plainly.")
        .max_output_tokens(req.max_output_tokens)
        .temperature(0.0)
        .max_iterations(3);
    match req.mode.as_str() {
        "tool" => b = b.tool(Arc::new(FactTool { name: "lookup_fact", delay: Duration::ZERO, stats: req.stats.clone() })),
        "slowtool" => b = b.tool(Arc::new(FactTool { name: "slow_lookup", delay: Duration::from_secs(30), stats: req.stats.clone() })),
        _ => {}
    }
    let agent = Arc::new(b.build().map_err(failure)?);
    let sessions: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
    let sid = "s".to_string();
    sessions.create(CreateRequest { app_name: "memex".into(), user_id: "u".into(), session_id: Some(sid.clone()), state: Default::default() }).await.map_err(failure)?;
    let runner = Runner::builder().app_name("memex").agent(agent as Arc<dyn Agent>).session_service(sessions).cancellation_token(req.token).build().map_err(failure)?;
    let cfg = RunConfig::builder().streaming_mode(StreamingMode::SSE).build();
    let stream = runner
        .run_with_config(UserId::new("u").map_err(identity)?, SessionId::new(sid).map_err(identity)?, Content::new("user").with_text(&req.prompt), Some(cfg))
        .await
        .map_err(failure)?;
    let stats = req.stats;
    Ok(Box::pin(stream.map(move |ev| {
        let ev = ev.map_err(failure)?;
        if let Some(code) = &ev.llm_response.error_code {
            // Providers can also report failure inside a successful chunk.
            return Err(RunFailure { code: "provider".into(), detail: format!("{code}: {}", ev.llm_response.error_message.clone().unwrap_or_default()) });
        }
        let event = to_runner_event(&ev);
        if !event.partial {
            let full: String = event.parts.iter().filter_map(|p| if let RunnerPart::Text(t) = p { Some(t.as_str()) } else { None }).collect();
            if !full.is_empty() { stats.lock().unwrap().record_final(&event.turn, &full); }
        }
        Ok(event)
    })))
}
