//! Edge: the only crate that depends on ADK-Rust. One schema-only call with no tools, in plain types, so no ADK type leaves this file.
//!
//! The binary wraps this behind `memex_core::AgentRunner`, so this crate doesn't depend on core.
mod chat;

use adk_agent::LlmAgentBuilder;
use adk_core::{Agent, Content, ErrorCategory, FinishReason, Llm, Part, RunConfig, SessionId, StreamingMode, UserId};
use adk_model::gemini::{GeminiModel, ThinkingConfig, ThinkingLevel};
use adk_model::openai_compatible::{OpenAICompatible, OpenAICompatibleConfig};
use adk_runner::Runner;
use adk_session::{CreateRequest, InMemorySessionService, SessionService};
use futures::StreamExt;
use memex_types::{AgentError, AgentReply, AgentRequest, AgentUsage};
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmEndpoint {
    Gemini {
        api_key: String,
        model: String,
    },
    /// A local or other server speaking the OpenAI API.
    OpenAi {
        base_url: String,
        api_key: Option<String>,
        model: String,
    },
}

pub struct AdkAgent {
    runtime: tokio::runtime::Runtime,
    llm: Arc<dyn Llm>,
    model: String,
    secret: Option<String>,
    counter: AtomicU64,
    native_schema: bool,
    /// How often a running chat looks for a cancel.
    chat_tick: Duration,
}

/// What Gemini's own `response_schema` takes. It refuses anything else with a 400, so the rest is left out of what's sent, and the full schema is still enforced on the reply by the caller.
const GEMINI_KEYWORDS: [&str; 8] = ["type", "format", "description", "nullable", "enum", "required", "minItems", "maxItems"];

/// Rewrites a schema into Gemini's subset. It walks schema positions, so a field named like a keyword survives. Anything that ends up saying nothing becomes a string, as Gemini refuses an empty schema.
pub fn gemini_schema(schema: &Value) -> Value {
    let Value::Object(m) = schema else { return json!({"type": "string"}) };
    let mut out = Map::new();
    for (k, v) in m {
        match k.as_str() {
            "properties" => {
                let inner: Map<String, Value> = v
                    .as_object()
                    .map(|p| p.iter().map(|(n, s)| (n.clone(), gemini_schema(s))).collect())
                    .unwrap_or_default();
                if !inner.is_empty() {
                    out.insert(k.clone(), Value::Object(inner));
                }
            }
            "items" => {
                out.insert(k.clone(), gemini_schema(v));
            }
            "type" => {
                // A list of types, as in ["string", "null"], is one type that may be null.
                match v {
                    Value::Array(types) => {
                        if let Some(first) = types.iter().find(|t| t.as_str() != Some("null")) {
                            out.insert("type".into(), first.clone());
                        }
                        if types.iter().any(|t| t.as_str() == Some("null")) {
                            out.insert("nullable".into(), json!(true));
                        }
                    }
                    other => {
                        out.insert(k.clone(), other.clone());
                    }
                }
            }
            name if GEMINI_KEYWORDS.contains(&name) => {
                out.insert(k.clone(), v.clone());
            }
            _ => {}
        }
    }
    // An object with no properties is refused, so it's asked for as text and the caller's check decides.
    let empty_object = out.get("type").and_then(Value::as_str) == Some("object") && !out.contains_key("properties");
    if !out.contains_key("type") || empty_object {
        return json!({"type": "string"});
    }
    if let Some(Value::Array(required)) = out.get("required") {
        let known: Vec<Value> = required
            .iter()
            .filter(|name| name.as_str().is_some_and(|n| out.get("properties").and_then(|p| p.get(n)).is_some()))
            .cloned()
            .collect();
        out.insert("required".into(), Value::Array(known));
    }
    Value::Object(out)
}

fn categorize(e: adk_core::AdkError, secret: Option<&str>) -> AgentError {
    let text = scrub(e.to_string(), secret);
    match e.category {
        ErrorCategory::Unavailable => AgentError::Unavailable(text),
        ErrorCategory::RateLimited => AgentError::RateLimited(text),
        ErrorCategory::Timeout => AgentError::Timeout,
        _ if text.contains("output schema validation failed") => AgentError::SchemaInvalid {
            detail: text,
            usage: AgentUsage::default(),
        },
        _ => AgentError::Provider(text),
    }
}

/// An error can quote a request's URL, and these messages end up in job rows.
fn scrub(text: String, secret: Option<&str>) -> String {
    match secret {
        Some(s) if !s.is_empty() => text.replace(s, "<redacted>"),
        _ => text,
    }
}

impl AdkAgent {
    pub fn new(endpoint: LlmEndpoint) -> Result<Self, String> {
        let (llm, model, secret, native_schema): (Arc<dyn Llm>, String, Option<String>, bool) = match endpoint {
            LlmEndpoint::Gemini { api_key, model } => {
                // Spec 007: extraction turns thinking off, and on Gemini the lowest level is the nearest there is.
                let client = GeminiModel::new(api_key.clone(), model.clone())
                    .map_err(|e| scrub(e.to_string(), Some(&api_key)))?
                    .with_thinking_config(ThinkingConfig::new().with_thinking_level(ThinkingLevel::Minimal));
                (Arc::new(client), model, Some(api_key), true)
            }
            LlmEndpoint::OpenAi { base_url, api_key, model } => {
                let key = api_key.clone().unwrap_or_else(|| "unused".to_string());
                let config = OpenAICompatibleConfig::new(key, model.clone())
                    .with_provider_name("memex")
                    .with_base_url(base_url);
                let client = OpenAICompatible::new(config)
                    .map_err(|e| scrub(e.to_string(), api_key.as_deref()))?
                    .with_request_adapter(Arc::new(|body, _headers| {
                        // Spec 007: with thinking on, the same prompt took eight times as long and tokens.
                        body["chat_template_kwargs"] = json!({"enable_thinking": false});
                        Ok(())
                    }));
                (Arc::new(client), model, api_key, false)
            }
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            chat_tick: Duration::from_millis(25),
            runtime,
            llm,
            model,
            secret,
            counter: AtomicU64::new(0),
            native_schema,
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    fn map(&self, e: adk_core::AdkError) -> AgentError {
        categorize(e, self.secret.as_deref())
    }

    async fn call(&self, req: &AgentRequest) -> Result<AgentReply, AgentError> {
        let schema: Value = serde_json::from_str(&req.schema_json).map_err(|e| AgentError::Provider(e.to_string()))?;
        let sent = if self.native_schema { gemini_schema(&schema) } else { schema };
        // No tools and no sub-agents. ADK's own schema retry is off, as retrying is core's policy and all tokens get counted there.
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
                .map_err(|e| self.map(e))?,
        );
        let sessions: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let session = format!("s{}", self.counter.fetch_add(1, Ordering::Relaxed));
        sessions
            .create(CreateRequest {
                app_name: "memex".into(),
                user_id: "extractor".into(),
                session_id: Some(session.clone()),
                state: Default::default(),
            })
            .await
            .map_err(|e| self.map(e))?;
        let runner = Runner::builder()
            .app_name("memex")
            .agent(agent as Arc<dyn Agent>)
            .session_service(sessions)
            .build()
            .map_err(|e| self.map(e))?;
        let config = RunConfig::builder().streaming_mode(StreamingMode::None).build();
        let user = UserId::new("extractor").map_err(|e| AgentError::Provider(e.to_string()))?;
        let session = SessionId::new(session).map_err(|e| AgentError::Provider(e.to_string()))?;
        let mut stream = runner
            .run_with_config(user, session, Content::new("user").with_text(&req.input), Some(config))
            .await
            .map_err(|e| self.map(e))?;
        let (mut text, mut usage, mut tool_calls, mut truncated) = (String::new(), AgentUsage::default(), 0u32, false);
        while let Some(event) = stream.next().await {
            // ADK checks a reply against the schema itself and ends the stream with this error, after the reply's usage has gone by. Those tokens are spent, so they stay on the error.
            let event = event.map_err(|e| match self.map(e) {
                AgentError::SchemaInvalid { .. } if truncated => AgentError::Truncated { usage },
                AgentError::SchemaInvalid { .. } if text.trim().is_empty() => AgentError::EmptyReply { usage },
                AgentError::SchemaInvalid { detail, .. } => AgentError::SchemaInvalid { detail, usage },
                other => other,
            })?;
            if let Some(u) = &event.llm_response.usage_metadata {
                usage = AgentUsage {
                    input_tokens: u.prompt_token_count.max(0) as u64,
                    output_tokens: u.candidates_token_count.max(0) as u64,
                    thinking_tokens: u.thinking_token_count.unwrap_or(0).max(0) as u64,
                };
            }
            if matches!(event.llm_response.finish_reason, Some(FinishReason::MaxTokens)) {
                truncated = true;
            }
            if let Some(content) = &event.llm_response.content {
                for part in &content.parts {
                    match part {
                        Part::Text { text: t } => text.push_str(t),
                        Part::FunctionCall { .. } => tool_calls += 1,
                        _ => {}
                    }
                }
            }
        }
        if truncated {
            return Err(AgentError::Truncated { usage });
        }
        if text.trim().is_empty() {
            return Err(AgentError::EmptyReply { usage });
        }
        Ok(AgentReply {
            json: text.trim().to_string(),
            usage,
            tool_calls,
        })
    }

    /// Blocks until the reply, or the request's own time limit.
    pub fn run(&self, req: &AgentRequest) -> Result<AgentReply, AgentError> {
        self.runtime.block_on(async {
            match tokio::time::timeout(Duration::from_millis(req.budget.timeout_ms), self.call(req)).await {
                Ok(result) => result,
                Err(_) => Err(AgentError::Timeout),
            }
        })
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // A stand-in server, which waits on real sockets.
mod tests {
    use super::*;
    use memex_types::AgentBudget;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    fn request(timeout_ms: u64) -> AgentRequest {
        AgentRequest {
            instruction: "Extract.".into(),
            input: "{\"segments\":[]}".into(),
            schema_json: json!({"type": "object", "properties": {"claims": {"type": "array"}}}).to_string(),
            budget: AgentBudget {
                max_output_tokens: 100,
                timeout_ms,
            },
        }
    }

    /// A one-file stand-in for an OpenAI-compatible server on this machine, so the wire mapping is tested with no LLM and nothing leaving the machine.
    struct Server {
        url: String,
        seen: Arc<Mutex<Vec<String>>>,
    }

    fn serve(status: &'static str, body: String) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(vec![]));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
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
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if data.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                log.lock().unwrap().push(String::from_utf8_lossy(&data).into_owned());
                let reply = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        Server { url, seen }
    }

    fn agent(url: &str) -> AdkAgent {
        AdkAgent::new(LlmEndpoint::OpenAi {
            base_url: url.into(),
            api_key: Some("test-key-not-real".into()),
            model: "m".into(),
        })
        .unwrap()
    }

    /// ADK always asks for a stream, so the stand-in sends one, cut in two as a real server would.
    fn completion(content: &str, finish: &str) -> String {
        let cut = content.len() / 2;
        let chunk = |delta: Value, finish: Value, usage: Value| json!({"id": "1", "object": "chat.completion.chunk", "created": 1, "model": "m", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}], "usage": usage});
        format!(
            "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            chunk(json!({"role": "assistant", "content": &content[..cut]}), Value::Null, Value::Null),
            chunk(json!({"content": &content[cut..]}), Value::Null, Value::Null),
            chunk(
                json!({}),
                json!(finish),
                json!({"prompt_tokens": 123, "completion_tokens": 45, "total_tokens": 168})
            ),
        )
    }

    #[test]
    fn a_reply_comes_back_with_its_usage_and_the_request_offers_no_tools() {
        let server = serve("200 OK", completion("{\"claims\": []}", "stop"));
        let reply = agent(&server.url).run(&request(30_000)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&reply.json).unwrap(), json!({"claims": []}));
        assert_eq!((reply.usage.input_tokens, reply.usage.output_tokens, reply.tool_calls), (123, 45, 0));
        let seen = server.seen.lock().unwrap().join("\n");
        let body = seen.split("\r\n\r\n").nth(1).unwrap();
        let sent: Value = serde_json::from_str(body).unwrap();
        assert!(sent.get("tools").is_none_or(|t| t.as_array().is_some_and(Vec::is_empty)), "{sent}");
        assert_eq!(sent["chat_template_kwargs"]["enable_thinking"], json!(false));
        assert!(body.contains("segments"), "the input is sent");
    }

    #[test]
    fn a_cut_off_reply_is_truncated_and_an_empty_one_is_empty() {
        let cut = serve("200 OK", completion("{\"claims\": [", "length"));
        let e = agent(&cut.url).run(&request(30_000)).unwrap_err();
        assert!(matches!(e, AgentError::Truncated { usage } if usage.input_tokens == 123), "{e:?}");
        let empty = serve("200 OK", completion("  ", "stop"));
        let e = agent(&empty.url).run(&request(30_000)).unwrap_err();
        assert!(matches!(e, AgentError::EmptyReply { usage } if usage.output_tokens == 45), "{e:?}");
        // A reply that isn't the schema's shape keeps its tokens too.
        let wrong = serve("200 OK", completion("[1, 2]", "stop"));
        let e = agent(&wrong.url).run(&request(30_000)).unwrap_err();
        assert!(matches!(e, AgentError::SchemaInvalid { usage, .. } if usage.input_tokens == 123), "{e:?}");
    }

    #[test]
    fn an_endpoint_that_is_down_is_an_outage() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        drop(listener);
        let e = agent(&url).run(&request(60_000)).unwrap_err();
        assert!(e.is_outage(), "{e:?}");
    }

    #[test]
    fn a_server_error_is_an_outage_and_a_refusal_is_not() {
        let error = json!({"error": {"message": "boom"}}).to_string();
        let broken = serve("503 Service Unavailable", error.clone());
        assert!(agent(&broken.url).run(&request(60_000)).unwrap_err().is_outage());
        let refused = serve("400 Bad Request", error);
        let e = agent(&refused.url).run(&request(60_000)).unwrap_err();
        assert!(matches!(e, AgentError::Provider(_)), "{e:?}");
    }

    #[test]
    fn a_slow_call_stops_at_its_limit() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        // It accepts and never answers.
        let keep = std::thread::spawn(move || {
            let held = listener.accept();
            std::thread::sleep(Duration::from_millis(500));
            drop(held);
        });
        assert_eq!(agent(&url).run(&request(50)).unwrap_err(), AgentError::Timeout);
        let _ = keep.join();
    }

    #[test]
    fn an_error_never_carries_the_key() {
        assert_eq!(scrub("bad url ?key=abc123".into(), Some("abc123")), "bad url ?key=<redacted>");
        assert_eq!(scrub("fine".into(), None), "fine");
    }

    #[test]
    fn gemini_gets_only_what_it_takes_and_fields_keep_their_names() {
        let schema = json!({"type": "object", "additionalProperties": false, "required": ["a", "gone"], "properties": {
            "a": {"type": "string", "pattern": "x", "minLength": 1},
            "additionalProperties": {"type": "number"},
            "maybe": {"type": ["string", "null"]},
            "open": {},
            "bag": {"type": "object"},
            "list": {"type": "array", "items": {"type": "object", "additionalProperties": false, "properties": {"b": {"type": "integer", "minimum": 1}}}},
        }});
        let sent = gemini_schema(&schema);
        assert!(sent.get("additionalProperties").is_none());
        assert_eq!(sent["required"], json!(["a"]));
        assert_eq!(sent["properties"]["a"], json!({"type": "string"}));
        assert_eq!(sent["properties"]["additionalProperties"], json!({"type": "number"}));
        assert_eq!(sent["properties"]["maybe"], json!({"type": "string", "nullable": true}));
        assert_eq!(sent["properties"]["open"], json!({"type": "string"}));
        assert_eq!(sent["properties"]["bag"], json!({"type": "string"}));
        assert_eq!(sent["properties"]["list"]["items"]["properties"]["b"], json!({"type": "integer"}));
    }
}
