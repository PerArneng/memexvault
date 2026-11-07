//! Frontend: MCP on `rmcp`. Tools parse their arguments, call the same `Server` facade and DTOs as HTTP, and format. No rules live here.
//!
//! Tool definitions come from `tools.json`, a file compiled in, so nothing in a vault can reach them.
mod handler;
mod http_route;
mod resources;
mod respond;
mod tools;

pub use handler::{Credential, McpServer, serve_stdio};
pub use http_route::{MAX_REQUEST_BYTES, MCP_PATH, McpHttp, routes};
pub use respond::{DATA_NOTICE, DEFAULT_MAX_RESPONSE_BYTES};
pub use tools::{INSTRUCTIONS, MAX_INGEST_BYTES, TOOLS_JSON, tool_definitions};
