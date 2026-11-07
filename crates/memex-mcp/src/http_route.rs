//! Streamable HTTP at one route, stateless, with the API's bearer tokens. A client gets JSON answers and holds no session, so the server keeps nothing per client.
use crate::handler::{Credential, McpServer};
use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use memex_api::ErrorBody;
use memex_core::{Server, ServerEdges, ServerError};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use std::sync::Arc;

pub const MCP_PATH: &str = "/api/v1/mcp";
/// Room for the largest `ingest`: 8 MiB of content comes to about 10.7 MiB as base64.
pub const MAX_REQUEST_BYTES: usize = 12 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct McpHttp {
    /// Host names a request may name, which stops DNS rebinding reaching a server on this machine.
    pub allowed_hosts: Vec<String>,
    pub max_response_bytes: usize,
}

/// A bearer token decides alone. Cookies aren't accepted here, so a browser's session can never reach MCP. The caller is checked on every request, so revoking a token takes effect at once.
async fn authenticate<E>(State(server): State<Arc<Server<E>>>, mut req: Request, next: Next) -> Response
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let header_value = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok());
    let token = header_value.and_then(|v| v.strip_prefix("Bearer ")).map(str::to_string);
    let check = server.clone();
    let caller = tokio::task::spawn_blocking(move || check.authenticate(token.as_deref().ok_or(ServerError::Unauthenticated)?)).await;
    match caller {
        Ok(Ok(c)) => {
            req.extensions_mut().insert(c);
            next.run(req).await
        }
        Ok(Err(e)) => refuse(&server, &e),
        Err(_) => refuse(&server, &ServerError::Unavailable("authentication panicked".into())),
    }
}

fn refuse<E>(server: &Server<E>, e: &ServerError) -> Response
where
    E: ServerEdges,
{
    let status = match e {
        ServerError::Unauthenticated => StatusCode::UNAUTHORIZED,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    let body = ErrorBody {
        error: memex_api::error_detail(e, &server.new_request_id()),
    };
    let mut res = (status, Json(body)).into_response();
    if status == StatusCode::UNAUTHORIZED {
        res.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    }
    res
}

/// The route, for the composition root to serve beside the API.
pub fn routes<E, S>(server: Arc<Server<E>>, config: McpHttp) -> axum::Router<S>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
    S: Clone + Send + Sync + 'static,
{
    let max = config.max_response_bytes;
    let for_tools = server.clone();
    let service = StreamableHttpService::new(
        move || Ok(McpServer::new(for_tools.clone(), Credential::PerRequest, max)),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_allowed_hosts(config.allowed_hosts)
            .with_max_request_body_bytes(MAX_REQUEST_BYTES),
    );
    axum::Router::new()
        .route_service(MCP_PATH, service)
        .layer(middleware::from_fn_with_state(server, authenticate::<E>))
}
