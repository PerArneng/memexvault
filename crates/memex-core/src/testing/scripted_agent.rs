//! An agent for tests, so the machinery around a model runs with no model.
type AgentReplyFn = Box<dyn Fn(&memex_types::AgentRequest) -> Result<memex_types::AgentReply, memex_types::AgentError> + Send + Sync>;

/// An agent that plays back a script of replies, and then answers with `then` if it has one, or as an outage. It keeps every request. It never stands in for a model's quality: its replies are whatever the test wrote.
pub struct ScriptedAgent {
    script: std::sync::Mutex<std::collections::VecDeque<Result<memex_types::AgentReply, memex_types::AgentError>>>,
    requests: std::sync::Mutex<Vec<memex_types::AgentRequest>>,
    then: Option<AgentReplyFn>,
}

impl ScriptedAgent {
    pub fn new(script: Vec<Result<memex_types::AgentReply, memex_types::AgentError>>) -> Self {
        Self {
            script: std::sync::Mutex::new(script.into()),
            requests: std::sync::Mutex::new(vec![]),
            then: None,
        }
    }

    /// Answers every request by calling `f`.
    pub fn answering(f: impl Fn(&memex_types::AgentRequest) -> Result<memex_types::AgentReply, memex_types::AgentError> + Send + Sync + 'static) -> Self {
        let mut agent = Self::new(vec![]);
        agent.then = Some(Box::new(f));
        agent
    }

    /// Once the script runs out, answers by calling `f`.
    pub fn then(mut self, f: impl Fn(&memex_types::AgentRequest) -> Result<memex_types::AgentReply, memex_types::AgentError> + Send + Sync + 'static) -> Self {
        self.then = Some(Box::new(f));
        self
    }

    pub fn requests(&self) -> Vec<memex_types::AgentRequest> {
        self.requests.lock().expect("lock").clone()
    }
}

impl crate::AgentRunner for ScriptedAgent {
    fn model(&self) -> String {
        "scripted".into()
    }

    fn run(&self, req: &memex_types::AgentRequest) -> Result<memex_types::AgentReply, memex_types::AgentError> {
        self.requests.lock().expect("lock").push(req.clone());
        if let Some(next) = self.script.lock().expect("lock").pop_front() {
            return next;
        }
        match &self.then {
            Some(f) => f(req),
            None => Err(memex_types::AgentError::Unavailable("the script ran out".into())),
        }
    }
}

/// One thing a scripted model call does, in order.
#[derive(Debug, Clone)]
pub enum ChatStep {
    /// A partial event with this text.
    Text(String),
    /// Asks for a tool, runs it through the host as a real run would, and sends its result on.
    Tool { name: String, args: serde_json::Value },
    /// Waits until the host says it's cancelled. It stands for a call still generating.
    UntilCancelled,
    /// The provider fails here, keeping what it has spent so far.
    Fail(memex_types::AgentError),
}

/// One model call: what it says it needs, and what it does. `usage` of none means the provider doesn't report any.
#[derive(Debug, Clone)]
pub struct ChatCall {
    pub input_estimate: u64,
    pub steps: Vec<ChatStep>,
    pub usage: Option<memex_types::AgentUsage>,
    pub truncated: bool,
}

impl ChatCall {
    pub fn new(steps: Vec<ChatStep>) -> Self {
        Self {
            input_estimate: 100,
            steps,
            usage: Some(memex_types::AgentUsage {
                input_tokens: 90,
                output_tokens: 10,
                thinking_tokens: 0,
            }),
            truncated: false,
        }
    }

    /// A call that only says this.
    pub fn says(text: &str) -> Self {
        Self::new(vec![ChatStep::Text(text.into())])
    }

    /// A call that only asks for this tool.
    pub fn calls(name: &str, args: serde_json::Value) -> Self {
        Self::new(vec![ChatStep::Tool { name: name.into(), args }])
    }
}

/// A chat model made of a script, one list of calls per run. It asks the host about each call, as the contract says, and keeps what it was asked and given. It never stands in for a model's judgement: its choices are whatever the test wrote.
#[derive(Default)]
pub struct ScriptedChat {
    runs: std::sync::Mutex<std::collections::VecDeque<Vec<ChatCall>>>,
    requests: std::sync::Mutex<Vec<memex_types::ChatRequest>>,
    /// The output limit the host gave each call.
    limits: std::sync::Mutex<Vec<u32>>,
    /// What each tool call returned, as the model would see it.
    pub tool_answers: std::sync::Mutex<Vec<serde_json::Value>>,
}

impl ScriptedChat {
    pub fn new(runs: Vec<Vec<ChatCall>>) -> Self {
        Self {
            runs: std::sync::Mutex::new(runs.into()),
            ..Self::default()
        }
    }

    pub fn requests(&self) -> Vec<memex_types::ChatRequest> {
        self.requests.lock().expect("lock").clone()
    }

    /// How many model calls were really made, which is how a test knows nothing reached the LLM.
    pub fn limits(&self) -> Vec<u32> {
        self.limits.lock().expect("lock").clone()
    }

    pub fn tool_answers(&self) -> Vec<serde_json::Value> {
        self.tool_answers.lock().expect("lock").clone()
    }
}

impl crate::AgentRunner for ScriptedChat {
    fn model(&self) -> String {
        "scripted-chat".into()
    }

    fn run(&self, _req: &memex_types::AgentRequest) -> Result<memex_types::AgentReply, memex_types::AgentError> {
        Err(memex_types::AgentError::Provider("this script only chats".into()))
    }

    #[allow(clippy::disallowed_methods)] // It stands in for a model waiting on the network.
    fn chat(&self, req: &memex_types::ChatRequest, host: std::sync::Arc<dyn memex_types::ChatHost>) -> memex_types::ChatEnd {
        use memex_types::{AgentUsage, ChatEnd, ChatStop, RunnerEvent, RunnerPart, estimate_tokens};
        self.requests.lock().expect("lock").push(req.clone());
        let calls = self.runs.lock().expect("lock").pop_front().unwrap_or_default();
        let mut truncated = false;
        for (n, call) in calls.into_iter().enumerate() {
            if n as u32 >= req.max_model_calls {
                return ChatEnd::Stopped(ChatStop::Turns);
            }
            let limit = match host.before_call(call.input_estimate) {
                Ok(limit) => limit,
                Err(stop) => return ChatEnd::Stopped(stop),
            };
            self.limits.lock().expect("lock").push(limit);
            let turn = format!("t{n}");
            let (mut chars, mut text, mut tools) = (0, String::new(), vec![]);
            let estimated = |chars: usize| AgentUsage {
                input_tokens: call.input_estimate,
                output_tokens: estimate_tokens(chars),
                thinking_tokens: 0,
            };
            for step in &call.steps {
                match step {
                    ChatStep::Text(t) => {
                        chars += t.len();
                        text.push_str(t);
                        host.event(RunnerEvent {
                            turn: turn.clone(),
                            partial: true,
                            parts: vec![RunnerPart::Text(t.clone())],
                        });
                    }
                    ChatStep::Tool { name, args } => {
                        let id = format!("{turn}-c{}", tools.len());
                        tools.push(RunnerPart::ToolCall {
                            id: Some(id.clone()),
                            name: name.clone(),
                            args: args.clone(),
                        });
                    }
                    ChatStep::UntilCancelled => {
                        while !host.cancelled() {
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                        host.after_call(estimated(chars), true);
                        return ChatEnd::Stopped(ChatStop::Cancelled);
                    }
                    ChatStep::Fail(e) => {
                        host.after_call(estimated(chars), true);
                        return ChatEnd::Failed(e.clone());
                    }
                }
            }
            // A terminal event repeats the call's text and tool calls, as ADK's does.
            let mut parts = vec![];
            if !text.is_empty() {
                parts.push(RunnerPart::Text(text));
            }
            parts.extend(tools.iter().cloned());
            host.event(RunnerEvent {
                turn: turn.clone(),
                partial: false,
                parts,
            });
            match call.usage {
                Some(usage) => host.after_call(usage, false),
                None => host.after_call(estimated(chars), true),
            }
            truncated = call.truncated;
            for tool in tools {
                let RunnerPart::ToolCall { id, name, args } = tool else { continue };
                let result = host.call_tool(&name, &args);
                self.tool_answers.lock().expect("lock").push(result.clone());
                if host.cancelled() {
                    return ChatEnd::Stopped(ChatStop::Cancelled);
                }
                host.event(RunnerEvent {
                    turn: format!("{turn}-r"),
                    partial: false,
                    parts: vec![RunnerPart::ToolResult { id, name, result }],
                });
            }
        }
        ChatEnd::Finished { truncated }
    }
}
