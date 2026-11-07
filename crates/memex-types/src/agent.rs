//! The plain types of an agent call. They cross the `AgentRunner` boundary, so no agent library's type appears here.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What one call may spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentBudget {
    pub max_output_tokens: u32,
    pub timeout_ms: u64,
}

/// One schema-only call: no tools, no sub-agents, and no state between calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRequest {
    pub instruction: String,
    pub input: String,
    /// The JSON Schema the reply must satisfy.
    pub schema_json: String,
    pub budget: AgentBudget,
}

/// Token counts as the provider reported them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
}

impl AgentUsage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.thinking_tokens
    }

    pub fn add(&mut self, other: &AgentUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.thinking_tokens += other.thinking_tokens;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReply {
    pub json: String,
    pub usage: AgentUsage,
    /// Should always be 0, as no tools are offered. A reply that tries anyway is refused.
    pub tool_calls: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentError {
    /// The endpoint is down or unreachable. The job waits and tries later.
    Unavailable(String),
    RateLimited(String),
    Timeout,
    /// The model returned no text, for example only reasoning.
    EmptyReply {
        usage: AgentUsage,
    },
    /// The model hit its token limit before finishing.
    Truncated {
        usage: AgentUsage,
    },
    /// Text came back that doesn't satisfy the schema.
    SchemaInvalid {
        detail: String,
        usage: AgentUsage,
    },
    /// The provider refused the request, and trying the same thing again won't help soon.
    Provider(String),
}

impl AgentError {
    /// The LLM is away rather than wrong, so waiting is the right answer and uses up no attempts.
    pub fn is_outage(&self) -> bool {
        matches!(self, Self::Unavailable(_) | Self::RateLimited(_) | Self::Timeout)
    }
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "the LLM is unavailable: {m}"),
            Self::RateLimited(m) => write!(f, "the LLM is rate limiting: {m}"),
            Self::Timeout => f.write_str("the LLM call timed out"),
            Self::EmptyReply { .. } => f.write_str("the model returned no text"),
            Self::Truncated { .. } => f.write_str("the model hit its token limit before finishing"),
            Self::SchemaInvalid { detail, .. } => write!(f, "the reply doesn't match the schema: {detail}"),
            Self::Provider(m) => write!(f, "the LLM provider refused the call: {m}"),
        }
    }
}

/// Tokens from a length of text, erring high. Search budgets use 4 characters per token, and charging for what wasn't measured wants more margin than that.
pub fn estimate_tokens(chars: usize) -> u64 {
    (chars as u64).div_ceil(3)
}

/// A tool the agent may call. These are compiled in, and never built from anything in a vault.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// Its arguments, as JSON Schema.
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatTurn {
    pub from_user: bool,
    pub text: String,
}

/// One run of the chat agent: a new message, what came before, and what it may call.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub instruction: String,
    /// Oldest first. Only what the user and the agent said, so tool results are never sent twice.
    pub history: Vec<ChatTurn>,
    pub message: String,
    pub tools: Vec<ToolDefinition>,
    /// The most model calls this run makes.
    pub max_model_calls: u32,
    /// The whole run's limit.
    pub timeout_ms: u64,
}

/// A part of what the runner emits. A partial event holds only what's new, and a terminal one repeats the whole model call so far.
#[derive(Debug, Clone, PartialEq)]
pub enum RunnerPart {
    Text(String),
    ToolCall { id: Option<String>, name: String, args: Value },
    ToolResult { id: Option<String>, name: String, result: Value },
}

/// One streaming event. `turn` is stable for one model call.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunnerEvent {
    pub turn: String,
    pub partial: bool,
    pub parts: Vec<RunnerPart>,
}

/// Why a run ended early with nothing having gone wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatStop {
    ThreadBudget,
    DayBudget,
    Cancelled,
    /// It used all its model calls without finishing.
    Turns,
    /// The thread was deleted or erased while the run was going.
    ThreadGone,
    /// An admin turned chat off for this vault, or the caller's access ended, while the run was going.
    Refused,
}

impl ChatStop {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ThreadBudget => "thread_budget",
            Self::DayBudget => "day_budget",
            Self::Cancelled => "cancelled",
            Self::Turns => "turns",
            Self::ThreadGone => "thread_gone",
            Self::Refused => "refused",
        }
    }
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEnd {
    /// `truncated` means the last reply hit its token limit.
    Finished {
        truncated: bool,
    },
    Stopped(ChatStop),
    Failed(AgentError),
}

/// What a chat run needs from its caller while it goes. The edge holds this for the length of a run, so it stays free of any store or token.
pub trait ChatHost: Send + Sync {
    /// Asked before every model call, with an estimate of its input that errs high. Says the most it may write, or why the call mustn't be made.
    fn before_call(&self, input_estimate: u64) -> Result<u32, ChatStop>;
    /// Called once for each call `before_call` allowed, however it ended. `estimated` means the provider didn't report these numbers, and they err high.
    fn after_call(&self, usage: AgentUsage, estimated: bool);
    /// Blocks while the tool runs. A failure comes back as data for the model to read.
    fn call_tool(&self, name: &str, args: &Value) -> Value;
    fn event(&self, event: RunnerEvent);
    fn cancelled(&self) -> bool;
}
