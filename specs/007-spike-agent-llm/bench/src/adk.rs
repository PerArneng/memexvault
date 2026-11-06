//! The edge: AgentRunner on ADK-Rust. No ADK type leaves this file.
use crate::runner::{AgentError, AgentReply, AgentRequest, AgentRunner, Usage};
use adk_agent::LlmAgentBuilder;
use adk_core::{Agent, Content, ErrorCategory, FinishReason, Llm, Part, RunConfig, SessionId, StreamingMode, UserId};
use adk_model::gemini::GeminiModel;
use adk_model::openai_compatible::{OpenAICompatible, OpenAICompatibleConfig};
use adk_runner::Runner;
use adk_session::{CreateRequest, InMemorySessionService, SessionService};
use futures::StreamExt;
use std::{sync::{atomic::{AtomicU64, Ordering}, Arc}, time::Duration};

pub enum Provider {
    Gemini { key: String, model: String },
    Local { base_url: String, model: String, thinking: bool },
}

pub struct AdkAgentRunner {
    rt: tokio::runtime::Runtime,
    llm: Arc<dyn Llm>,
    n: AtomicU64,
    strip_additional: bool,
}

/// Gemini's native response_schema rejects `additionalProperties`. It's removed from what's sent, and our validator still enforces it.
fn strip(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(m) => { m.remove("additionalProperties"); m.values_mut().for_each(strip); }
        serde_json::Value::Array(a) => a.iter_mut().for_each(strip),
        _ => {}
    }
}

impl AdkAgentRunner {
    pub fn new(p: Provider) -> Result<Self, String> {
        let strip_additional = matches!(p, Provider::Gemini { .. });
        let llm: Arc<dyn Llm> = match p {
            Provider::Gemini { key, model } => Arc::new(GeminiModel::new(key, model).map_err(|e| e.to_string())?),
            Provider::Local { base_url, model, thinking } => {
                let cfg = OpenAICompatibleConfig::new("unused", model).with_provider_name("vllm").with_base_url(base_url);
                let mut c = OpenAICompatible::new(cfg).map_err(|e| e.to_string())?;
                if !thinking {
                    c = c.with_request_adapter(Arc::new(|body, _headers| {
                        body["chat_template_kwargs"] = serde_json::json!({"enable_thinking": false});
                        Ok(())
                    }));
                }
                Arc::new(c)
            }
        };
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().map_err(|e| e.to_string())?;
        Ok(Self { rt, llm, n: AtomicU64::new(0), strip_additional })
    }

    async fn go(&self, req: &AgentRequest) -> Result<AgentReply, AgentError> {
        let schema: serde_json::Value = serde_json::from_str(&req.schema_json).map_err(|e| AgentError::Provider(e.to_string()))?;
        let mut sent = schema.clone();
        if self.strip_additional { strip(&mut sent); }
        // No tools, no sub-agents, schema only. ADK's own schema retry is off, since retry is our policy.
        let agent = Arc::new(
            LlmAgentBuilder::new("extractor")
                .model(self.llm.clone())
                .instruction(req.instruction.clone())
                .output_schema(sent)
                .output_max_retries(0)
                .max_output_tokens(req.budget.max_output_tokens as i32)
                .temperature(0.0)
                .max_iterations(1)
                .build()
                .map_err(|e| AgentError::Provider(e.to_string()))?,
        );
        let sessions: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let sid = format!("s{}", self.n.fetch_add(1, Ordering::Relaxed));
        sessions.create(CreateRequest { app_name: "memex".into(), user_id: "u".into(), session_id: Some(sid.clone()), state: Default::default() }).await.map_err(map)?;
        let runner = Runner::builder().app_name("memex").agent(agent as Arc<dyn Agent>).session_service(sessions).build().map_err(map)?;
        let cfg = RunConfig::builder().streaming_mode(StreamingMode::None).build();
        let mut stream = runner
            .run_with_config(UserId::new("u").map_err(|e| AgentError::Provider(e.to_string()))?, SessionId::new(sid).map_err(|e| AgentError::Provider(e.to_string()))?, Content::new("user").with_text(&req.input), Some(cfg))
            .await
            .map_err(map)?;
        let (mut text, mut usage, mut tool_calls, mut truncated) = (String::new(), Usage::default(), 0u32, false);
        while let Some(ev) = stream.next().await {
            let ev = ev.map_err(map)?;
            if let Some(u) = &ev.llm_response.usage_metadata {
                usage = Usage { input_tokens: u.prompt_token_count.max(0) as u64, output_tokens: u.candidates_token_count.max(0) as u64, thinking_tokens: u.thinking_token_count.unwrap_or(0).max(0) as u64 };
            }
            if matches!(ev.llm_response.finish_reason, Some(FinishReason::MaxTokens)) { truncated = true; }
            if let Some(c) = &ev.llm_response.content {
                for p in &c.parts {
                    match p {
                        Part::Text { text: t } => text.push_str(t),
                        Part::FunctionCall { .. } => tool_calls += 1,
                        _ => {}
                    }
                }
            }
        }
        if truncated { return Err(AgentError::Truncated { usage }); }
        if text.trim().is_empty() { return Err(AgentError::EmptyReply { usage }); }
        let doc: serde_json::Value = serde_json::from_str(text.trim()).map_err(|e| AgentError::SchemaInvalid { detail: e.to_string(), usage: usage.clone() })?;
        if let Err(e) = jsonschema::validate(&schema, &doc) {
            return Err(AgentError::SchemaInvalid { detail: e.to_string(), usage });
        }
        Ok(AgentReply { json: text, usage, tool_calls })
    }
}

fn map(e: adk_core::AdkError) -> AgentError {
    let m = e.to_string();
    match e.category {
        ErrorCategory::Unavailable => AgentError::Unavailable(m),
        ErrorCategory::RateLimited => AgentError::RateLimited(m),
        ErrorCategory::Timeout => AgentError::Timeout,
        _ if m.contains("output schema validation failed") => AgentError::SchemaInvalid { detail: m, usage: Usage::default() },
        _ => AgentError::Provider(m),
    }
}

impl AgentRunner for AdkAgentRunner {
    fn run(&self, req: &AgentRequest) -> Result<AgentReply, AgentError> {
        self.rt.block_on(async {
            match tokio::time::timeout(Duration::from_millis(req.budget.timeout_ms), self.go(req)).await {
                Ok(r) => r,
                Err(_) => Err(AgentError::Timeout),
            }
        })
    }
}
