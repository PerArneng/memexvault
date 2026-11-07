//! Routes for the activity stream and the live board.
use super::{App, RequestId, authed, list};
use crate::activity::{self, Hub, TooManyStreams};
use crate::error::ApiError;
use axum::extract::{ConnectInfo, Extension, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use memex_api::{ActivityDocumentPage, ActivityDocumentParams, ActivityParams, ErrorBody, ListParams};
use memex_core::activity::resume_request;
use memex_core::{Server, ServerEdges, ServerError};
use memex_types::{ObjectId, Stage, VaultId};
use std::net::SocketAddr;
use std::sync::Arc;
use utoipa::OpenApi;

const LAST_EVENT_ID: &str = "last-event-id";

#[derive(OpenApi)]
#[openapi(
    paths(activity_stream, activity_documents),
    components(schemas(
        memex_api::DurableFrame,
        memex_api::SourceFact,
        memex_api::JobFact,
        memex_api::RenditionFact,
        memex_api::ExtractionFact,
        memex_api::DerivationFact,
        memex_api::ChangeSetFact,
        memex_api::AuditRunFact,
        memex_api::ProgressFrame,
        memex_api::ResyncFrame,
        memex_api::HelloFrame,
        memex_api::HeartbeatFrame,
        memex_api::EndFrame,
        memex_api::DocumentProgressDto,
        memex_api::DocumentActivityDto,
        ActivityDocumentPage
    ))
)]
pub(super) struct ActivityDoc;

pub(super) fn add_routes<E>(api: Router<App<E>>) -> Router<App<E>>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    api.route("/vaults/{id}/activity", get(activity_stream::<E>))
        .route("/vaults/{id}/activity/documents", get(activity_documents::<E>))
}

/// Follows everything happening in a vault as server-sent events. Frames are named `hello`, `durable`, `progress`, `resync`, `heartbeat` and `end`. `durable` and `resync` frames have ids, and a stream resumes after one with `Last-Event-ID` or `after`. A `resync` frame means fetch a new snapshot from `/activity/documents` and carry on. Frames hold ids, kinds, stages, counts and timings, and never document text.
#[utoipa::path(
    get,
    path = "/api/v1/vaults/{id}/activity",
    tag = "activity",
    params(("id" = String, Path), ("Last-Event-ID" = Option<String>, Header, description = "The last frame id seen. A position that can't be resumed from gives a `resync` frame, not an error."), ActivityParams),
    responses(
        (status = 200, content_type = "text/event-stream", body = String, description = "A stream of frames, each with the schema of its name: `DurableFrame`, `ProgressFrame`, `ResyncFrame`, `HelloFrame`, `HeartbeatFrame` or `EndFrame`"),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 429, body = ErrorBody, description = "This principal has as many streams open as it may, with the code `too_many_streams`")
    )
)]
async fn activity_stream<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    Extension(hub): Extension<Arc<Hub>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    Path(id): Path<String>,
    Query(params): Query<ActivityParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let vault = VaultId(id);
    let v = vault.clone();
    let (principal, marks) = authed(server.clone(), headers.clone(), &rid, move |s, c| {
        s.activity_allowed(c, &v)?;
        let marks = s.activity_marks(&v).map_err(ServerError::from)?;
        Ok((c.principal.id.clone(), marks))
    })
    .await?;
    let limits = server.activity_limits();
    let slot = hub.take_slot(&principal, limits.max_streams).map_err(|TooManyStreams(limit)| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_streams",
            format!("this principal already has {limit} activity streams open, which is the limit, so close one first"),
            &rid.0,
        )
    })?;
    let last_event_id = headers.get(LAST_EVENT_ID).map(|v| v.to_str().unwrap_or("not text").to_string());
    let request = resume_request(last_event_id.as_deref(), params.after.as_deref());
    let head = marks.head.map_or(-1, |h| h as i64);
    let opened = hub.open(&server, &vault, head, marks.generation, &headers, request);
    let peer = peer.map(|Extension(ConnectInfo(addr))| addr);
    if let Some(addr) = peer {
        hub.watch_connection(addr, limits.stall_ms);
    }
    let mut res = activity::body_of(hub, server, vault, opened, slot, peer).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // A proxy in front mustn't hold frames back until it has a bufferful.
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(res)
}

/// The live board: each document's stage and status, in id order. `head` is the log position this reflects, to follow the stream from.
#[utoipa::path(
    get,
    path = "/api/v1/vaults/{id}/activity/documents",
    tag = "activity",
    params(("id" = String, Path), ActivityDocumentParams),
    responses((status = 200, body = ActivityDocumentPage), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody), (status = 404, body = ErrorBody))
)]
async fn activity_documents<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
    Query(params): Query<ActivityDocumentParams>,
    headers: HeaderMap,
) -> Result<Json<ActivityDocumentPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(
        ListParams {
            cursor: params.cursor,
            limit: params.limit,
            sort: params.sort,
        },
        &rid,
    )?;
    let stage = match params.stage.as_deref() {
        None => None,
        Some(text) => Some(Stage::parse(text).ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid", format!("'{text}' isn't a stage"), &rid.0))?),
    };
    let vault = VaultId(id);
    let (after, limit) = (l.cursor.map(ObjectId), l.limit);
    let snapshot = authed(server, headers, &rid, move |s, c| {
        s.activity_documents(c, &vault, stage, after.as_ref(), limit + 1)
    })
    .await?;
    Ok(Json(memex_api::activity_page(snapshot, limit)))
}
