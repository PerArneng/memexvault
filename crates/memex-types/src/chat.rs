//! What the chat agent keeps and sends. Threads stay out of the event log, so that deleting one leaves no copy behind.
use crate::ObjectId;
use crate::rows::Header;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One vault's chat setting, the row's only id. It's a state change like any other, so it's on the log with who made it.
pub const CHAT_SETTINGS_ID: &str = "chat";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSettingsRow {
    pub header: Header,
    /// Off until an admin says otherwise, because it sends vault content to the LLM.
    pub enabled: bool,
    /// None takes the server's default.
    pub thread_token_budget: Option<u64>,
    pub day_token_budget: Option<u64>,
    pub updated_by: String,
    pub updated_at_ms: i64,
}

/// A conversation. It belongs to one principal, whose id is part of its key, so nobody can reach another's thread by naming its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatThread {
    pub owner: String,
    pub id: String,
    pub title: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// Everything this thread's runs have spent or reserved.
    pub tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    User,
    Assistant,
    Tool,
}

/// A tool call, with what it answered once it has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatToolCall {
    pub id: String,
    pub name: String,
    pub args: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub run_id: String,
    pub role: ChatRole,
    /// What was said, or a tool's answer.
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ChatToolCall>,
    /// Why this reply ended early, when it did, such as `cancelled` or `max_tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<String>,
    pub at_ms: i64,
}

/// A message, with its place in its thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatStored {
    pub seq: u64,
    pub message: ChatMessage,
}

/// The answer to reserving tokens. Nothing is charged unless it's `Reserved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatReservation {
    Reserved,
    ThreadOver,
    DayOver,
    /// The thread has been deleted, so the run should stop.
    ThreadGone,
}

/// A thread's owner and id, which only together name a thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChatThreadKey {
    pub owner: String,
    pub id: String,
}

/// A thread erasing a source removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatErasure {
    pub source: ObjectId,
    pub threads: Vec<ChatThreadKey>,
}

/// One AG-UI event, as `@ag-ui/core` 1.0.4 defines it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", rename_all_fields = "camelCase")]
pub enum AgUi {
    RunStarted {
        thread_id: String,
        run_id: String,
    },
    TextMessageStart {
        message_id: String,
        role: &'static str,
    },
    TextMessageContent {
        message_id: String,
        delta: String,
    },
    TextMessageEnd {
        message_id: String,
    },
    ToolCallStart {
        tool_call_id: String,
        tool_call_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_message_id: Option<String>,
    },
    ToolCallArgs {
        tool_call_id: String,
        delta: String,
    },
    ToolCallEnd {
        tool_call_id: String,
    },
    ToolCallResult {
        message_id: String,
        tool_call_id: String,
        content: String,
        role: &'static str,
    },
    RunFinished {
        thread_id: String,
        run_id: String,
        /// Why it ended early, when it did. Plain `RUN_FINISHED` can't tell a client its reply was cut.
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
    },
    RunError {
        message: String,
        code: String,
    },
}
