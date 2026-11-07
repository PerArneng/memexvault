//! The plain types of an agent call. They cross the `AgentRunner` boundary, so no agent library's type appears here.
use serde::{Deserialize, Serialize};

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
