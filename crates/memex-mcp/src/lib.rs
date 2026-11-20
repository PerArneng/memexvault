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

/// How a client connects, for the admin console (spec 039): the endpoint, transports, protocol versions and the tools, from the same definitions the server answers with.
pub fn connection_info(allowed_hosts: &[String]) -> memex_api::McpInfoDto {
    use rmcp::model::ProtocolVersion;
    memex_api::McpInfoDto {
        endpoint: MCP_PATH.into(),
        transports: vec!["streamable-http".into(), "stdio".into()],
        protocol_version: ProtocolVersion::LATEST.to_string(),
        supported_protocol_versions: ProtocolVersion::KNOWN_VERSIONS.iter().map(ToString::to_string).collect(),
        allowed_hosts: allowed_hosts.to_vec(),
        tools: tool_definitions()
            .iter()
            .map(|t| memex_api::McpToolDto {
                name: t.name.to_string(),
                description: t.description.as_deref().unwrap_or_default().to_string(),
            })
            .collect(),
    }
}
