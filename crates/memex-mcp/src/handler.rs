//! The MCP server: what `rmcp` calls. It finds the caller, moves to a blocking thread as HTTP does, and reports failures in the API's error format.
use crate::tools::{self, INSTRUCTIONS};
use crate::{resources, respond};
use memex_api::ErrorBody;
use memex_core::{Caller, Server, ServerEdges, ServerError};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, ListResourceTemplatesResult, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities, ServerConfig,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData, ServerHandler, ServiceExt};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// How often `min_seq` checks again while it waits, as over HTTP.
const MIN_SEQ_POLL: Duration = Duration::from_millis(50);

/// Who's asking.
#[derive(Clone)]
pub enum Credential {
    /// Streamable HTTP: each request's bearer token, checked before the request gets here.
    PerRequest,
    /// Stdio: one token from settings, checked at every call, so revoking it ends access as it does over HTTP.
    Token(Arc<String>),
}

pub struct McpServer<E: ServerEdges> {
    server: Arc<Server<E>>,
    credential: Credential,
    max_response_bytes: usize,
}

impl<E: ServerEdges> Clone for McpServer<E> {
    fn clone(&self) -> Self {
        Self {
            server: self.server.clone(),
            credential: self.credential.clone(),
            max_response_bytes: self.max_response_bytes,
        }
    }
}

fn error_text(e: &ServerError, request_id: &str) -> String {
    let body = ErrorBody {
        error: memex_api::error_detail(e, request_id),
    };
    serde_json::to_string(&body).unwrap_or_default()
}

fn from_request(context: &RequestContext<RoleServer>) -> Option<Caller> {
    context.extensions.get::<http::request::Parts>()?.extensions.get::<Caller>().cloned()
}

impl<E> McpServer<E>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    pub fn new(server: Arc<Server<E>>, credential: Credential, max_response_bytes: usize) -> Self {
        Self {
            server,
            credential,
            max_response_bytes,
        }
    }

    /// Runs `f` on a blocking thread as the caller. A task that panics is reported like any other backend failure.
    async fn as_caller<T, F>(&self, context: &RequestContext<RoleServer>, f: F) -> Result<T, ServerError>
    where
        T: Send + 'static,
        F: FnOnce(&Server<E>, &Caller) -> Result<T, ServerError> + Send + 'static,
    {
        let from_http = from_request(context);
        let (server, credential) = (self.server.clone(), self.credential.clone());
        let done = tokio::task::spawn_blocking(move || {
            let caller = match credential {
                Credential::PerRequest => from_http.ok_or(ServerError::Unauthenticated)?,
                Credential::Token(t) => server.authenticate(&t)?,
            };
            f(&server, &caller)
        })
        .await;
        done.unwrap_or_else(|_| Err(ServerError::Unavailable("a tool call panicked".into())))
    }
}

impl<E> ServerHandler for McpServer<E>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_resources().build());
        info.server_info = Implementation::new("memexvault", env!("CARGO_PKG_VERSION"));
        info.instructions = Some(INSTRUCTIONS.into());
        info
    }

    async fn list_tools(&self, _request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools::tool_definitions().to_vec()))
    }

    async fn call_tool(&self, request: CallToolRequestParams, context: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
        let name = request.name.to_string();
        if !tools::known(&name) {
            let all: Vec<&str> = tools::tool_definitions().iter().map(|t| t.name.as_ref()).collect();
            return Err(ErrorData::invalid_params(
                format!("unknown tool '{name}'. The tools are: {}", all.join(", ")),
                None,
            ));
        }
        let args = Value::Object(request.arguments.unwrap_or_default());
        let wait = Duration::from_millis(tools::wait_allowed(&name, &args));
        let deadline = tokio::time::Instant::now() + wait;
        let max = self.max_response_bytes;
        loop {
            let (tool, a) = (name.clone(), args.clone());
            let done = self.as_caller(&context, move |s, c| tools::call(s, c, &tool, a, max)).await;
            match done {
                Ok(answer) => return Ok(CallToolResult::success(vec![ContentBlock::text(answer.text())]).into()),
                // An index catches up on its own, so this is the one failure worth waiting out.
                Err(ServerError::NotCaughtUp { .. }) if tokio::time::Instant::now() + MIN_SEQ_POLL < deadline => tokio::time::sleep(MIN_SEQ_POLL).await,
                Err(e) => {
                    let request_id = self.server.new_request_id();
                    return Ok(CallToolResult::error(vec![ContentBlock::text(error_text(&e, &request_id))]).into());
                }
            }
        }
    }

    async fn list_resources(&self, _request: Option<PaginatedRequestParams>, _context: RequestContext<RoleServer>) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(resources::fixed()))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(resources::templates())
    }

    async fn read_resource(&self, request: ReadResourceRequestParams, context: RequestContext<RoleServer>) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri.clone();
        let max = self.max_response_bytes;
        let done = self.as_caller(&context, move |s, c| resources::read(s, c, &uri, max)).await;
        let data = |e: &ServerError, id: &str| -> Option<Value> { serde_json::to_value(memex_api::error_detail(e, id)).ok() };
        match done {
            Ok(Some(found)) => Ok(found.into()),
            Ok(None) => Err(ErrorData::resource_not_found(
                "no resource looks like that: try memex://vaults, memex://vaults/{vault}/pages/{id} or memex://vaults/{vault}/sources/{id}",
                Some(json!({"uri": request.uri})),
            )),
            Err(e) => {
                let id = self.server.new_request_id();
                let detail = data(&e, &id);
                let message = memex_api::error_message(&e, &id);
                Err(match e {
                    ServerError::NotFound => ErrorData::resource_not_found(message, detail),
                    ServerError::Invalid(_) | ServerError::Forbidden | ServerError::Unauthenticated => ErrorData::invalid_params(message, detail),
                    _ => ErrorData::internal_error(message, detail),
                })
            }
        }
    }
}

/// Serves one client over this process's stdin and stdout until the client leaves.
pub async fn serve_stdio<E>(server: Arc<Server<E>>, token: String) -> Result<(), String>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let running = McpServer::new(server, Credential::Token(Arc::new(token)), respond::DEFAULT_MAX_RESPONSE_BYTES)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| e.to_string())?;
    running.waiting().await.map(|_| ()).map_err(|e| e.to_string())
}
