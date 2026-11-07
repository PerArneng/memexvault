//! Chat's routes: a run over server-sent AG-UI events, a vault's setting, and your own threads.
use super::{App, RequestId, authed, body};
use crate::error::ApiError;
use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::stream;
use memex_api::{ChatRenameBody, ChatSettingsBody, ChatSettingsDto, ChatThreadDetail, ChatThreadDto, ChatThreadPage, ErrorBody, ListParams, RunAgentBody};
use memex_core::{AgentRunner, ChatInput, ChatSettingsUpdate, Server, ServerEdges, ServerError, new_chat_cancel};
use memex_types::{AgUi, VaultId};
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use utoipa::OpenApi;

/// Comments on a quiet stream, which `HttpAgent` reads through, so a slow tool doesn't look like a dead connection.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

#[derive(OpenApi)]
#[openapi(
    paths(chat_run_doc, get_settings, put_settings, list_threads, get_thread, rename_thread, delete_thread),
    components(schemas(
        memex_api::RunAgentBody,
        memex_api::ChatSettingsBody,
        memex_api::ChatSettingsDto,
        memex_api::ChatThreadDto,
        memex_api::ChatThreadPage,
        memex_api::ChatRenameBody,
        memex_api::ChatToolCallDto,
        memex_api::ChatMessageDto,
        memex_api::ChatThreadDetail
    ))
)]
pub(super) struct ChatDoc;

pub(super) fn add_routes<E>(api: Router<App<E>>) -> Router<App<E>>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    api.route("/vaults/{id}/chat/settings", get(get_settings::<E>).put(put_settings::<E>))
        .route("/vaults/{id}/chat/threads", get(list_threads::<E>))
        .route(
            "/vaults/{id}/chat/threads/{tid}",
            get(get_thread::<E>).patch(rename_thread::<E>).delete(delete_thread::<E>),
        )
}

/// The route that runs the agent. The composition root adds it beside the API with the model it built, as the server itself holds no agent. With none, a run says so.
pub fn chat_routes<E, A>(agent: Option<Arc<A>>) -> Router<App<E>>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
    A: AgentRunner + Send + Sync + 'static,
{
    Router::new().route(
        "/api/v1/vaults/{id}/chat/runs",
        post(
            move |State(server): State<App<E>>,
                  Extension(rid): Extension<RequestId>,
                  headers: HeaderMap,
                  Path(id): Path<String>,
                  parsed: Result<Json<RunAgentBody>, JsonRejection>| {
                let agent = agent.clone();
                async move { chat_run(server, agent, rid, headers, id, parsed).await }
            },
        ),
    )
}

/// Dropped with the response stream, which is how a client going away arrives. It has to live inside the stream's state: a guard that the stream's closure only borrowed from would be dropped straight away, and cancel every run.
struct StopWhenDropped(Arc<AtomicBool>);

impl Drop for StopWhenDropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn frame(event: &AgUi) -> Result<Event, Infallible> {
    Ok(Event::default().data(serde_json::to_string(event).expect("an AG-UI event serializes")))
}

async fn chat_run<E, A>(
    server: App<E>,
    agent: Option<Arc<A>>,
    rid: RequestId,
    headers: HeaderMap,
    id: String,
    parsed: Result<Json<RunAgentBody>, JsonRejection>,
) -> Result<Response, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
    A: AgentRunner + Send + Sync + 'static,
{
    let b = body(parsed, &rid)?;
    let message = b.message().map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, "invalid", m, &rid.0))?;
    let input = ChatInput {
        thread_id: b.thread_id,
        run_id: b.run_id,
        message,
    };
    let cancel = new_chat_cancel();
    let flag = cancel.clone();
    let has_agent = agent.is_some();
    // Every refusal happens here, as an ordinary error, before a stream opens or anything reaches the LLM.
    let prepared = authed(server.clone(), headers, &rid, move |s, c| {
        let prepared = s.prepare_chat(c, &VaultId(id), input, flag)?;
        if has_agent {
            Ok(prepared)
        } else {
            Err(ServerError::Unavailable(
                "this server has no chat model configured, so nothing can answer".into(),
            ))
        }
    })
    .await?;
    let Some(agent) = agent else {
        return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "no chat model", &rid.0));
    };
    // Unbounded, because a run holds no lock on this and its size is already bounded by its budget and tool limits, while a full channel would stall the model's stream behind one slow client.
    let (sender, receiver) = mpsc::unbounded_channel::<AgUi>();
    let run_server = server.clone();
    // The run isn't tied to this request's task, so what has been said gets kept even when the client goes.
    tokio::task::spawn_blocking(move || {
        run_server.run_chat(prepared, agent.as_ref(), memex_api::chat_tool::<E>, move |event| sender.send(event).is_ok());
    });
    let events = stream::unfold((receiver, StopWhenDropped(cancel)), |(mut receiver, guard)| async move {
        let event = receiver.recv().await?;
        Some((frame(&event), (receiver, guard)))
    });
    let mut response = Sse::new(events).keep_alive(KeepAlive::new().interval(KEEP_ALIVE)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
    Ok(response)
}

/// Runs the agent on one message, as AG-UI events over server-sent events. Anything that refuses it comes back as an ordinary error before the stream opens. A failure afterwards is a `RUN_ERROR` with a code, and a budget, a cancel or a reply cut at its limit is a `RUN_FINISHED` whose `result.stop` says so. Disconnecting cancels the run, and it can't be resumed. Needs read scope, and the agent can propose only with propose scope. A thread is the caller's own, and its id is theirs to choose, so a new id starts a new thread.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/chat/runs", tag = "chat", params(("id" = String, Path)), request_body = memex_api::RunAgentBody, responses((status = 200, description = "AG-UI events", content_type = "text/event-stream", body = String), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody), (status = 429, body = ErrorBody), (status = 503, body = ErrorBody)))]
#[allow(dead_code)]
async fn chat_run_doc() {}

fn settings_dto<E: ServerEdges>(s: &Server<E>, c: &memex_core::Caller, v: &VaultId) -> Result<ChatSettingsDto, ServerError> {
    let (enabled, thread_token_budget, day_token_budget) = s.chat_settings(c, v)?;
    let (day_tokens_used, day_tokens_limit) = s.chat_day_spend(c, v)?;
    Ok(ChatSettingsDto {
        enabled,
        thread_token_budget,
        day_token_budget,
        day_tokens_used,
        day_tokens_limit,
    })
}

/// Whether chat is on, what it may spend and what it has spent today.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/chat/settings", tag = "chat", params(("id" = String, Path)), responses((status = 200, body = memex_api::ChatSettingsDto), (status = 404, body = ErrorBody)))]
async fn get_settings<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ChatSettingsDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    Ok(Json(authed(server, headers, &rid, move |s, c| settings_dto(s, c, &VaultId(id))).await?))
}

/// Turns chat on or off. It's off until an admin turns it on, because on means this vault's content goes to the configured LLM. Needs admin scope.
#[utoipa::path(put, path = "/api/v1/vaults/{id}/chat/settings", tag = "chat", params(("id" = String, Path)), request_body = memex_api::ChatSettingsBody, responses((status = 200, body = memex_api::ChatSettingsDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn put_settings<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    parsed: Result<Json<ChatSettingsBody>, JsonRejection>,
) -> Result<Json<ChatSettingsDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let dto = authed(server, headers, &rid, move |s, c| {
        let v = VaultId(id);
        s.set_chat_settings(
            c,
            &v,
            ChatSettingsUpdate {
                enabled: b.enabled,
                thread_token_budget: b.thread_token_budget,
                day_token_budget: b.day_token_budget,
            },
        )?;
        settings_dto(s, c, &v)
    })
    .await?;
    Ok(Json(dto))
}

/// Your own threads, the most recently updated first. Nobody can list another's, whatever their scope.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/chat/threads", tag = "chat", params(("id" = String, Path), ListParams), responses((status = 200, body = memex_api::ChatThreadPage), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn list_threads<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<ChatThreadPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = super::list(params, &rid)?;
    if let Some(cursor) = &l.cursor
        && memex_api::parse_thread_cursor(cursor).is_none()
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid",
            "that isn't a cursor this route gave out",
            &rid.0,
        ));
    }
    let limit = l.limit;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.list_chat_threads(c, &VaultId(id), l.cursor.as_deref().and_then(memex_api::parse_thread_cursor), limit + 1)
    })
    .await?;
    let (items, next_cursor) = memex_api::page_of(rows.into_iter().map(ChatThreadDto::from).collect(), limit, memex_api::thread_cursor);
    Ok(Json(ChatThreadPage { items, next_cursor }))
}

/// One of your threads with a page of its messages, oldest first. Anyone else's looks like one that doesn't exist.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/chat/threads/{tid}", tag = "chat", params(("id" = String, Path), ("tid" = String, Path), ListParams), responses((status = 200, body = memex_api::ChatThreadDetail), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn get_thread<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, tid)): Path<(String, String)>,
    Query(params): Query<ListParams>,
) -> Result<Json<ChatThreadDetail>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = super::list(params, &rid)?;
    let after = match &l.cursor {
        None => None,
        Some(cursor) => Some(
            cursor
                .parse::<u64>()
                .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "invalid", "that isn't a cursor this route gave out", &rid.0))?,
        ),
    };
    let limit = l.limit;
    let view = authed(server, headers, &rid, move |s, c| s.get_chat_thread(c, &VaultId(id), &tid, after, limit + 1)).await?;
    Ok(Json(ChatThreadDetail::new(view, limit)))
}

/// Renames one of your threads.
#[utoipa::path(patch, path = "/api/v1/vaults/{id}/chat/threads/{tid}", tag = "chat", params(("id" = String, Path), ("tid" = String, Path)), request_body = memex_api::ChatRenameBody, responses((status = 200, body = memex_api::ChatThreadDto), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn rename_thread<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, tid)): Path<(String, String)>,
    parsed: Result<Json<ChatRenameBody>, JsonRejection>,
) -> Result<Json<ChatThreadDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let row = authed(server, headers, &rid, move |s, c| s.rename_chat_thread(c, &VaultId(id), &tid, &b.title)).await?;
    Ok(Json(row.into()))
}

/// Deletes one of your threads and everything in it, and stops any run in it. Today's spending stays counted.
#[utoipa::path(delete, path = "/api/v1/vaults/{id}/chat/threads/{tid}", tag = "chat", params(("id" = String, Path), ("tid" = String, Path)), responses((status = 204), (status = 404, body = ErrorBody)))]
async fn delete_thread<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, tid)): Path<(String, String)>,
) -> Result<StatusCode, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    authed(server, headers, &rid, move |s, c| s.delete_chat_thread(c, &VaultId(id), &tid)).await?;
    Ok(StatusCode::NO_CONTENT)
}
