//! A chat run on ADK's `Runner`: streaming, with tools, on the path spec 034 proved. The host decides what each call may spend, runs the tools and takes the events, and no ADK type leaves this file.
use crate::{AdkAgent, categorize};
use adk_agent::LlmAgentBuilder;
use adk_core::{
    Agent, Content, Event, FinishReason, Llm, LlmRequest, LlmResponse, LlmResponseStream, Part, RunConfig, SessionId, StreamingMode, Tool, ToolContext, UserId,
};
use adk_runner::Runner;
use adk_session::{CreateRequest, InMemorySessionService, SessionService};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use memex_types::{AgentError, AgentUsage, ChatEnd, ChatHost, ChatRequest, ChatStop, RunnerEvent, RunnerPart, ToolDefinition, estimate_tokens};
use serde_json::Value;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

const AGENT: &str = "chat";

/// A call's first words: it costs a little, whatever the text.
const CALL_TOKENS: u64 = 16;
/// How long a cancelled run waits for its last call to report, so its Derivation has the number.
const SETTLE_WAIT: Duration = Duration::from_millis(250);

fn input_estimate(req: &LlmRequest) -> u64 {
    let contents = serde_json::to_string(&req.contents).map_or(0, |t| t.len());
    let tools: usize = req.tools.values().map(|t| t.to_string().len()).sum();
    estimate_tokens(contents + tools) + CALL_TOKENS
}

/// What one call has spent, which it tells the host when it's dropped, however the call ended. A stream ADK leaves early still gets here, which is why a dropped stream alone says nothing about a cancel.
struct Settle {
    host: Arc<dyn ChatHost>,
    input: u64,
    output_chars: usize,
    reported: AgentUsage,
    finished: bool,
    open: Arc<AtomicU32>,
}

impl Settle {
    fn see(&mut self, chunk: &LlmResponse) {
        if let Some(u) = &chunk.usage_metadata {
            self.reported = AgentUsage {
                input_tokens: u.prompt_token_count.max(0) as u64,
                output_tokens: u.candidates_token_count.max(0) as u64,
                thinking_tokens: u.thinking_token_count.unwrap_or(0).max(0) as u64,
            };
        }
        if chunk.finish_reason.is_some() || chunk.turn_complete {
            self.finished = true;
        }
        if !chunk.partial && self.output_chars > 0 {
            return; // The terminal chunk repeats what was already counted.
        }
        for part in chunk.content.iter().flat_map(|c| &c.parts) {
            self.output_chars += match part {
                Part::Text { text } => text.len(),
                Part::Thinking { thinking, .. } => thinking.len(),
                Part::FunctionCall { args, name, .. } => args.to_string().len() + name.len(),
                _ => 0,
            };
        }
    }

    /// The provider's numbers if it finished and gave any. Otherwise, as with a cancelled call or a provider that doesn't report, an estimate that errs high and is never zero.
    fn usage(&self) -> (AgentUsage, bool) {
        if self.finished && self.reported.total() > 0 {
            return (self.reported, false);
        }
        let usage = AgentUsage {
            input_tokens: self.reported.input_tokens.max(self.input),
            output_tokens: self.reported.output_tokens.max(estimate_tokens(self.output_chars)),
            thinking_tokens: self.reported.thinking_tokens,
        };
        (usage, true)
    }
}

impl Drop for Settle {
    fn drop(&mut self) {
        let (usage, estimated) = self.usage();
        let (host, open) = (self.host.clone(), self.open.clone());
        let settle = move || {
            host.after_call(usage, estimated);
            open.fetch_sub(1, Ordering::SeqCst);
        };
        // It writes to the store, which shouldn't hold up an async worker, and a drop can't wait.
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(settle);
            }
            Err(_) => settle(),
        }
    }
}

struct WatchedStream {
    inner: LlmResponseStream,
    settle: Settle,
}

impl Stream for WatchedStream {
    type Item = adk_core::Result<LlmResponse>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let polled = self.inner.as_mut().poll_next(cx);
        if let Poll::Ready(Some(Ok(chunk))) = &polled {
            self.settle.see(chunk);
        }
        polled
    }
}

/// The model, with the host asked before every call. Where a call can't fit, it isn't made.
struct ChatLlm {
    inner: Arc<dyn Llm>,
    host: Arc<dyn ChatHost>,
    stop: Arc<Mutex<Option<ChatStop>>>,
    open: Arc<AtomicU32>,
}

#[async_trait]
impl Llm for ChatLlm {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn schema_adapter(&self) -> &dyn adk_core::SchemaAdapter {
        self.inner.schema_adapter()
    }

    fn uses_interactions_api(&self) -> bool {
        self.inner.uses_interactions_api()
    }

    async fn generate_content(&self, mut req: LlmRequest, stream: bool) -> adk_core::Result<LlmResponseStream> {
        let input = input_estimate(&req);
        let host = self.host.clone();
        let allowed = tokio::task::spawn_blocking(move || host.before_call(input))
            .await
            .map_err(|e| adk_core::AdkError::model(e.to_string()))?;
        let limit = match allowed {
            Ok(limit) => limit,
            Err(stop) => {
                *self.stop.lock().expect("lock") = Some(stop);
                return Err(adk_core::AdkError::model("the host stopped this run"));
            }
        };
        self.open.fetch_add(1, Ordering::SeqCst);
        // Made before the call starts, so a cancel while it connects or retries still settles.
        let settle = Settle {
            host: self.host.clone(),
            input,
            output_chars: 0,
            reported: AgentUsage::default(),
            finished: false,
            open: self.open.clone(),
        };
        req.config.get_or_insert_with(Default::default).max_output_tokens = Some(limit as i32);
        let inner = self.inner.generate_content(req, stream).await?;
        Ok(Box::pin(WatchedStream { inner, settle }))
    }
}

/// A tool whose work and refusals are all the host's. An async worker doesn't block on a store call, and when a cancel drops this the work finishes unseen.
struct HostTool {
    definition: ToolDefinition,
    host: Arc<dyn ChatHost>,
}

#[async_trait]
impl Tool for HostTool {
    fn name(&self) -> &str {
        &self.definition.name
    }

    fn description(&self) -> &str {
        &self.definition.description
    }

    fn parameters_schema(&self) -> Option<Value> {
        Some(self.definition.parameters.clone())
    }

    async fn execute(&self, _ctx: Arc<dyn ToolContext>, args: Value) -> adk_core::Result<Value> {
        let (host, name) = (self.host.clone(), self.definition.name.clone());
        tokio::task::spawn_blocking(move || host.call_tool(&name, &args))
            .await
            .map_err(|e| adk_core::AdkError::tool(e.to_string()))
    }
}

/// Thinking and media parts go nowhere. Nothing a model thinks is shown or stored.
fn runner_event(ev: &Event) -> RunnerEvent {
    let parts = ev
        .llm_response
        .content
        .iter()
        .flat_map(|c| &c.parts)
        .filter_map(|p| match p {
            Part::Text { text } => Some(RunnerPart::Text(text.clone())),
            Part::FunctionCall { name, args, id, .. } => Some(RunnerPart::ToolCall {
                id: id.clone(),
                name: name.clone(),
                args: args.clone(),
            }),
            Part::FunctionResponse { function_response, id, .. } => Some(RunnerPart::ToolResult {
                id: id.clone(),
                name: function_response.name.clone(),
                result: function_response.response.clone(),
            }),
            _ => None,
        })
        .collect();
    RunnerEvent {
        turn: ev.id.clone(),
        partial: ev.llm_response.partial,
        parts,
    }
}

impl AdkAgent {
    /// Blocks until the run ends. A cancel drops the stream, which closes the provider's connection and is what stops its token use.
    pub fn chat(&self, req: &ChatRequest, host: Arc<dyn ChatHost>) -> ChatEnd {
        self.runtime.block_on(self.chat_run(req, host))
    }

    async fn chat_run(&self, req: &ChatRequest, host: Arc<dyn ChatHost>) -> ChatEnd {
        let stop = Arc::new(Mutex::new(None));
        let open = Arc::new(AtomicU32::new(0));
        let end = self.chat_stream(req, host.clone(), stop.clone(), open.clone()).await;
        let settled = async {
            while open.load(Ordering::SeqCst) > 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        };
        let _ = tokio::time::timeout(SETTLE_WAIT, settled).await;
        let stopped = *stop.lock().expect("lock");
        match (stopped, end) {
            (Some(stop), _) => ChatEnd::Stopped(stop),
            (None, end) => end,
        }
    }

    async fn chat_stream(&self, req: &ChatRequest, host: Arc<dyn ChatHost>, stop: Arc<Mutex<Option<ChatStop>>>, open: Arc<AtomicU32>) -> ChatEnd {
        let failed = |e: adk_core::AdkError| ChatEnd::Failed(categorize(e, self.secret.as_deref()));
        let llm: Arc<dyn Llm> = Arc::new(ChatLlm {
            inner: self.llm.clone(),
            host: host.clone(),
            stop: stop.clone(),
            open,
        });
        // No output guardrails: ADK streams only while there are none, as spec 034 found.
        let mut builder = LlmAgentBuilder::new(AGENT)
            .model(llm)
            .instruction(req.instruction.clone())
            .temperature(0.0)
            .max_iterations(req.max_model_calls);
        for definition in &req.tools {
            builder = builder.tool(Arc::new(HostTool {
                definition: definition.clone(),
                host: host.clone(),
            }));
        }
        let agent = match builder.build() {
            Ok(a) => Arc::new(a),
            Err(e) => return failed(e),
        };
        let sessions: Arc<dyn SessionService> = Arc::new(InMemorySessionService::new());
        let session = format!("c{}", self.counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let created = sessions
            .create(CreateRequest {
                app_name: "memex".into(),
                user_id: AGENT.into(),
                session_id: Some(session.clone()),
                state: Default::default(),
            })
            .await;
        if let Err(e) = created {
            return failed(e);
        }
        for turn in &req.history {
            let (author, role) = if turn.from_user { ("user", "user") } else { (AGENT, "model") };
            let mut event = Event::new("history");
            event.author = author.into();
            event.llm_response.content = Some(Content::new(role).with_text(&turn.text));
            if let Err(e) = sessions.append_event(&session, event).await {
                return failed(e);
            }
        }
        let runner = Runner::builder().app_name("memex").agent(agent as Arc<dyn Agent>).session_service(sessions).build();
        let runner = match runner {
            Ok(r) => r,
            Err(e) => return failed(e),
        };
        let (Ok(user), Ok(session)) = (UserId::new(AGENT), SessionId::new(session)) else {
            return ChatEnd::Failed(AgentError::Provider("bad session name".into()));
        };
        let config = RunConfig::builder().streaming_mode(StreamingMode::SSE).build();
        let events = runner.run_with_config(user, session, Content::new("user").with_text(&req.message), Some(config)).await;
        let mut events = match events {
            Ok(s) => s,
            Err(e) => return failed(e),
        };
        let deadline = tokio::time::sleep(Duration::from_millis(req.timeout_ms));
        tokio::pin!(deadline);
        let mut tick = tokio::time::interval(self.chat_tick);
        let mut truncated = false;
        let mut model_failure = None;
        loop {
            tokio::select! {
                biased;
                _ = tick.tick() => {
                    if host.cancelled() {
                        *stop.lock().expect("lock") = Some(ChatStop::Cancelled);
                        return ChatEnd::Stopped(ChatStop::Cancelled);
                    }
                }
                () = &mut deadline => return ChatEnd::Failed(AgentError::Timeout),
                item = events.next() => {
                    let Some(item) = item else { break };
                    let event = match item {
                        Ok(event) => event,
                        Err(e) if e.to_string().contains("Max iterations") => return ChatEnd::Stopped(ChatStop::Turns),
                        Err(e) => return failed(e),
                    };
                    if let Some(code) = &event.llm_response.error_code {
                        // A provider can report failure inside a successful chunk.
                        model_failure = Some(format!("{code}: {}", event.llm_response.error_message.clone().unwrap_or_default()));
                        break;
                    }
                    match event.llm_response.finish_reason {
                        Some(FinishReason::MaxTokens) => truncated = true,
                        Some(_) => truncated = false,
                        None => {}
                    }
                    let mapped = runner_event(&event);
                    if !mapped.parts.is_empty() {
                        host.event(mapped);
                    }
                }
            }
        }
        if let Some(failure) = model_failure {
            return ChatEnd::Failed(AgentError::Provider(failure));
        }
        ChatEnd::Finished { truncated }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_macros)] // A stand-in server, which waits on real sockets and the clock, and prints what it measured.
mod tests {
    use super::*;
    use crate::LlmEndpoint;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    #[derive(Default)]
    struct Host {
        events: Mutex<Vec<RunnerEvent>>,
        tools: Mutex<Vec<(String, Value)>>,
        before: Mutex<Vec<u64>>,
        after: Mutex<Vec<(AgentUsage, bool)>>,
        refuse: Mutex<Option<ChatStop>>,
        cancel: AtomicBool,
    }

    impl ChatHost for Host {
        fn before_call(&self, input_estimate: u64) -> Result<u32, ChatStop> {
            self.before.lock().unwrap().push(input_estimate);
            match *self.refuse.lock().unwrap() {
                Some(stop) => Err(stop),
                None => Ok(77),
            }
        }

        fn after_call(&self, usage: AgentUsage, estimated: bool) {
            self.after.lock().unwrap().push((usage, estimated));
        }

        fn call_tool(&self, name: &str, args: &Value) -> Value {
            self.tools.lock().unwrap().push((name.into(), args.clone()));
            json!({"data": "from the tool"})
        }

        fn event(&self, event: RunnerEvent) {
            self.events.lock().unwrap().push(event);
        }

        fn cancelled(&self) -> bool {
            self.cancel.load(Ordering::SeqCst)
        }
    }

    impl Host {
        fn text(&self) -> String {
            let events = self.events.lock().unwrap();
            let partial: String = events
                .iter()
                .filter(|e| e.partial)
                .flat_map(|e| &e.parts)
                .filter_map(|p| if let RunnerPart::Text(t) = p { Some(t.as_str()) } else { None })
                .collect();
            partial
        }
    }

    fn request(tools: Vec<ToolDefinition>) -> ChatRequest {
        ChatRequest {
            instruction: "Answer.".into(),
            history: vec![],
            message: "What is it?".into(),
            tools,
            max_model_calls: 3,
            timeout_ms: 30_000,
        }
    }

    fn search_tool() -> ToolDefinition {
        ToolDefinition {
            name: "search".into(),
            description: "Finds things.".into(),
            parameters: json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}),
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

    /// One reply per request, in order, with what each request said.
    fn script(replies: Vec<(&'static str, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(vec![]));
        let log = seen.clone();
        std::thread::spawn(move || {
            let mut replies = replies.into_iter();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                log.lock().unwrap().push(read_request(&mut stream));
                let (status, body) = replies.next().unwrap_or(("500 Internal Server Error", "{}".into()));
                let reply = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (url, seen)
    }

    fn agent(url: &str) -> AdkAgent {
        AdkAgent::new(LlmEndpoint::OpenAi {
            base_url: url.into(),
            api_key: Some("test-key-not-real".into()),
            model: "m".into(),
        })
        .unwrap()
    }

    fn chunk(delta: Value, finish: Value, usage: Value) -> String {
        let body = json!({"id": "1", "object": "chat.completion.chunk", "created": 1, "model": "m", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}], "usage": usage});
        format!("data: {body}\n\n")
    }

    fn usage(input: u64, output: u64) -> Value {
        json!({"prompt_tokens": input, "completion_tokens": output, "total_tokens": input + output})
    }

    fn text_reply(parts: &[&str], used: Value) -> String {
        let mut out = chunk(json!({"role": "assistant", "content": ""}), Value::Null, Value::Null);
        for p in parts {
            out += &chunk(json!({"content": p}), Value::Null, Value::Null);
        }
        out += &chunk(json!({}), json!("stop"), used);
        out + "data: [DONE]\n\n"
    }

    fn tool_reply(name: &str, args: &str, used: Value) -> String {
        let call = json!({"role": "assistant", "tool_calls": [{"index": 0, "id": "call-1", "type": "function", "function": {"name": name, "arguments": args}}]});
        chunk(call, Value::Null, Value::Null) + &chunk(json!({}), json!("tool_calls"), used) + "data: [DONE]\n\n"
    }

    fn settled(host: &Host, calls: usize) {
        let started = Instant::now();
        while host.after.lock().unwrap().len() < calls && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn text_arrives_as_deltas_with_the_providers_usage_and_the_host_sets_the_limit() {
        let (url, seen) = script(vec![("200 OK", text_reply(&["Hel", "lo ", "there"], usage(11, 3)))]);
        let host = Arc::new(Host::default());
        let end = agent(&url).chat(&request(vec![]), host.clone());
        assert_eq!(end, ChatEnd::Finished { truncated: false });
        assert_eq!(host.text(), "Hello there");
        settled(&host, 1);
        assert_eq!(
            host.after.lock().unwrap()[0],
            (
                AgentUsage {
                    input_tokens: 11,
                    output_tokens: 3,
                    thinking_tokens: 0
                },
                false
            )
        );
        let sent = seen.lock().unwrap().join("\n");
        assert!(sent.contains("\"max_tokens\":77") || sent.contains("\"max_completion_tokens\":77"), "{sent}");
        assert!(sent.contains("\"enable_thinking\":false"), "thinking stays off in chat: {sent}");
        assert!(sent.contains("\"stream\":true"), "{sent}");
    }

    #[test]
    fn a_tool_runs_through_the_host_and_the_model_then_answers_from_it() {
        let (url, seen) = script(vec![
            ("200 OK", tool_reply("search", "{\"q\":\"bridge\"}", usage(10, 2))),
            ("200 OK", text_reply(&["412 m"], usage(20, 2))),
        ]);
        let host = Arc::new(Host::default());
        let end = agent(&url).chat(&request(vec![search_tool()]), host.clone());
        assert_eq!(end, ChatEnd::Finished { truncated: false });
        assert_eq!(host.tools.lock().unwrap().clone(), vec![("search".to_string(), json!({"q": "bridge"}))]);
        settled(&host, 2);
        assert_eq!(host.before.lock().unwrap().len(), 2, "asked before each call");
        let events = host.events.lock().unwrap();
        assert!(events.iter().flat_map(|e| &e.parts).any(|p| matches!(p, RunnerPart::ToolResult { name, .. } if name == "search")));
        let sent = seen.lock().unwrap().join("\n");
        assert!(sent.contains("from the tool"), "the second call sees the tool's answer");
        assert!(sent.contains("\"tools\""), "{sent}");
    }

    #[test]
    fn history_reaches_the_model_in_order_and_before_the_new_message() {
        let (url, seen) = script(vec![("200 OK", text_reply(&["ok"], usage(1, 1)))]);
        let mut req = request(vec![]);
        req.history = vec![
            memex_types::ChatTurn {
                from_user: true,
                text: "first-question".into(),
            },
            memex_types::ChatTurn {
                from_user: false,
                text: "first-answer".into(),
            },
        ];
        let host = Arc::new(Host::default());
        assert_eq!(agent(&url).chat(&req, host), ChatEnd::Finished { truncated: false });
        let sent = seen.lock().unwrap().join("\n");
        let at = |s: &str| sent.find(s).unwrap_or_else(|| panic!("{s} wasn't sent: {sent}"));
        assert!(at("first-question") < at("first-answer") && at("first-answer") < at("What is it?"));
    }

    #[test]
    fn the_host_refusing_stops_the_run_with_no_request_to_the_model() {
        let (url, seen) = script(vec![]);
        let host = Arc::new(Host::default());
        *host.refuse.lock().unwrap() = Some(ChatStop::DayBudget);
        assert_eq!(agent(&url).chat(&request(vec![]), host.clone()), ChatEnd::Stopped(ChatStop::DayBudget));
        assert!(seen.lock().unwrap().is_empty());
        assert!(host.after.lock().unwrap().is_empty(), "a call that wasn't made costs nothing");
    }

    #[test]
    fn a_provider_that_reports_nothing_is_charged_an_estimate_that_is_not_zero() {
        let body = chunk(json!({"content": "some words"}), Value::Null, Value::Null) + &chunk(json!({}), json!("stop"), Value::Null) + "data: [DONE]\n\n";
        let (url, _) = script(vec![("200 OK", body)]);
        let host = Arc::new(Host::default());
        assert_eq!(agent(&url).chat(&request(vec![]), host.clone()), ChatEnd::Finished { truncated: false });
        settled(&host, 1);
        let (used, estimated) = host.after.lock().unwrap()[0];
        assert!(estimated);
        assert!(used.input_tokens > 0 && used.output_tokens >= estimate_tokens("some words".len()), "{used:?}");
    }

    #[test]
    fn a_reply_cut_at_its_limit_says_so() {
        let body = chunk(json!({"content": "half"}), Value::Null, Value::Null) + &chunk(json!({}), json!("length"), usage(5, 77)) + "data: [DONE]\n\n";
        let (url, _) = script(vec![("200 OK", body)]);
        assert_eq!(agent(&url).chat(&request(vec![]), Arc::new(Host::default())), ChatEnd::Finished { truncated: true });
    }

    #[test]
    fn failures_are_typed_and_carry_no_key() {
        let error = json!({"error": {"message": "boom test-key-not-real"}}).to_string();
        let (url, _) = script(vec![("400 Bad Request", error.clone())]);
        let end = agent(&url).chat(&request(vec![]), Arc::new(Host::default()));
        let ChatEnd::Failed(AgentError::Provider(text)) = end else { panic!("{end:?}") };
        assert!(!text.contains("test-key-not-real"), "{text}");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let down = format!("http://{}/v1", listener.local_addr().unwrap());
        drop(listener);
        let end = agent(&down).chat(&request(vec![]), Arc::new(Host::default()));
        assert!(matches!(&end, ChatEnd::Failed(e) if e.is_outage()), "{end:?}");
    }

    #[test]
    fn a_model_calling_a_tool_that_does_not_exist_reaches_nothing() {
        let canary = TcpListener::bind("127.0.0.1:0").unwrap();
        canary.set_nonblocking(true).unwrap();
        let fetch = format!("{{\"url\":\"http://{}/secret\"}}", canary.local_addr().unwrap());
        let (url, seen) = script(vec![("200 OK", tool_reply("fetch_url", &fetch, usage(5, 5))), ("200 OK", text_reply(&["done"], usage(5, 1)))]);
        let host = Arc::new(Host::default());
        let end = agent(&url).chat(&request(vec![search_tool()]), host.clone());
        assert_eq!(end, ChatEnd::Finished { truncated: false });
        assert!(host.tools.lock().unwrap().is_empty(), "the host was never asked");
        assert!(canary.accept().is_err(), "nothing connected to the URL");
        assert!(seen.lock().unwrap().join("\n").contains("not found"), "the model is told");
    }

    /// Item 7: from the cancel to the provider seeing its connection close. It keeps writing until a write fails, so that's when the upstream stops.
    #[test]
    fn a_cancel_closes_the_connection_to_the_provider_within_a_second() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let closed = Arc::new(Mutex::new(None));
        let saw = closed.clone();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else { return };
            read_request(&mut stream);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
            loop {
                let line = chunk(json!({"content": "word "}), Value::Null, Value::Null);
                if stream.write_all(line.as_bytes()).and_then(|()| stream.flush()).is_err() {
                    *saw.lock().unwrap() = Some(Instant::now());
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let host = Arc::new(Host::default());
        let (runner, canceller) = (host.clone(), host.clone());
        let at = Arc::new(Mutex::new(None));
        let cancelled_at = at.clone();
        let flip = std::thread::spawn(move || {
            while canceller.text().len() < 50 {
                std::thread::sleep(Duration::from_millis(2));
            }
            *cancelled_at.lock().unwrap() = Some(Instant::now());
            canceller.cancel.store(true, Ordering::SeqCst);
        });
        let end = agent(&url).chat(&request(vec![]), runner);
        let returned = Instant::now();
        flip.join().unwrap();
        assert_eq!(end, ChatEnd::Stopped(ChatStop::Cancelled));
        let cancelled = at.lock().unwrap().unwrap();
        let started = Instant::now();
        while closed.lock().unwrap().is_none() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        let upstream = closed.lock().unwrap().expect("the upstream saw its connection close");
        println!(
            "cancel to run returned {:?}, cancel to upstream closed {:?}",
            returned - cancelled,
            upstream.saturating_duration_since(cancelled)
        );
        assert!(upstream.saturating_duration_since(cancelled) < Duration::from_secs(1));
        // It never saw an end or a count, so it's charged an estimate covering what it saw, and never zero.
        settled(&host, 1);
        let (used, estimated) = host.after.lock().unwrap()[0];
        assert!(estimated);
        assert!(used.input_tokens > 0 && used.output_tokens >= estimate_tokens(50), "{used:?}");
    }
}
