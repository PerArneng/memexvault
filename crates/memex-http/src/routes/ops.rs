//! Routes for operating a vault (spec 030): erasure's preview, proposal and record, reconcile, reindex, the orphan sweep, and the server's status and metrics.
use super::{App, RequestId, authed, body, list};
use crate::error::ApiError;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use memex_api::{ErrorBody, ListParams, page_of};
use memex_core::{ReindexTarget, Server, ServerEdges};
use memex_types::{ObjectId, VaultId};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(erasure_preview, propose_erasure, list_erasures, reconcile, reindex, sweep, status, metrics),
    components(schemas(
        memex_api::RowKeyDto,
        memex_api::ErasureImpactDto,
        memex_api::ErasureDto,
        memex_api::ErasurePage,
        memex_api::EraseBody,
        memex_api::DigestDto,
        memex_api::IndexCheckDto,
        memex_api::ReconcileDto,
        memex_api::ReindexBody,
        memex_api::SweepBody,
        memex_api::SweepDto,
        memex_api::IndexStatusDto,
        memex_api::JobCountDto,
        memex_api::VaultStatusDto,
        memex_api::StatusDto
    ))
)]
pub(super) struct OpsDoc;

pub(super) fn add_routes<E>(api: Router<App<E>>) -> Router<App<E>>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    api.route("/vaults/{id}/sources/{sid}/erasure", get(erasure_preview::<E>).post(propose_erasure::<E>))
        .route("/vaults/{id}/erasures", get(list_erasures::<E>))
        .route("/vaults/{id}/reconcile", post(reconcile::<E>))
        .route("/vaults/{id}/reindex", post(reindex::<E>))
        .route("/vaults/{id}/sweep", post(sweep::<E>))
        .route("/status", get(status::<E>))
        .route("/metrics", get(metrics::<E>))
}

/// What erasing this document would remove, rewrite and flag, read now. The erasure's record lists the same once approved, unless the vault changed in between.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources/{sid}/erasure", tag = "operations", params(("id" = String, Path), ("sid" = String, Path)), responses((status = 200, body = memex_api::ErasureImpactDto), (status = 404, body = ErrorBody)))]
async fn erasure_preview<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
) -> Result<Json<memex_api::ErasureImpactDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let record = authed(server, headers, &rid, move |s, c| s.erasure_preview(c, &VaultId(id), &ObjectId(sid))).await?;
    Ok(Json((&record).into()))
}

/// Proposes erasing the document and every version of it. It always waits for a person, whoever proposes it, and only a person can approve it. Approving runs the erasure, which can't be undone.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/sources/{sid}/erasure", tag = "operations", params(("id" = String, Path), ("sid" = String, Path)), request_body = memex_api::EraseBody, responses((status = 200, body = memex_api::ProposalResult), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn propose_erasure<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
    parsed: Result<Json<memex_api::EraseBody>, JsonRejection>,
) -> Result<Json<memex_api::ProposalResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let out = authed(server, headers, &rid, move |s, c| {
        s.propose_erasure(c, &VaultId(id), &ObjectId(sid), b.rationale)
    })
    .await?;
    Ok(Json(memex_api::proposal_result(out)))
}

/// Erasures in the order they happened: what each removed, and the claims and pages each left flagged.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/erasures", tag = "operations", params(("id" = String, Path), ListParams), responses((status = 200, body = memex_api::ErasurePage), (status = 404, body = ErrorBody)))]
async fn list_erasures<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::ErasurePage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let req = list(params, &rid)?;
    let after = match req.cursor.as_deref().map(str::parse::<u64>) {
        None => None,
        Some(Ok(seq)) => Some(seq),
        Some(Err(_)) => return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid", "a bad cursor", &rid.0)),
    };
    let limit = req.limit;
    let views = authed(server, headers, &rid, move |s, c| s.erasures(c, &VaultId(id), after, limit + 1)).await?;
    let (items, next_cursor) = page_of(views, limit, |v| v.seq.to_string());
    Ok(Json(memex_api::ErasurePage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

/// Compares the log, the tables, the blobs and the indexes, with counts and hashes. With `repair`, rebuilds an index that differs. A vault admin's. It replays the log in memory, so on a large vault it takes a while.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/reconcile", tag = "operations", params(("id" = String, Path), memex_api::ReconcileQuery), responses((status = 200, body = memex_api::ReconcileDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn reconcile<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<memex_api::ReconcileQuery>,
) -> Result<Json<memex_api::ReconcileDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let (deep, repair) = (q.deep.unwrap_or(false), q.repair.unwrap_or(false));
    let report = authed(server, headers, &rid, move |s, c| s.reconcile(c, &VaultId(id), q.at, deep, repair)).await?;
    Ok(Json(report.into()))
}

/// Builds new index generations from the tables. The current ones keep serving until they're ready, and the status shows the build.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/reindex", tag = "operations", params(("id" = String, Path)), request_body = memex_api::ReindexBody, responses((status = 202), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn reindex<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    parsed: Result<Json<memex_api::ReindexBody>, JsonRejection>,
) -> Result<StatusCode, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let target = match b.index.as_str() {
        "keyword" => ReindexTarget::Keyword,
        "vector" => ReindexTarget::Vector,
        "all" => ReindexTarget::All,
        _ => return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid", "index is keyword, vector or all", &rid.0)),
    };
    authed(server, headers, &rid, move |s, c| s.reindex(c, &VaultId(id), target)).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Removes blobs no row refers to that are older than the grace period. The server also does this on its own schedule.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/sweep", tag = "operations", params(("id" = String, Path)), request_body = memex_api::SweepBody, responses((status = 200, body = memex_api::SweepDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn sweep<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    parsed: Result<Json<memex_api::SweepBody>, JsonRejection>,
) -> Result<Json<memex_api::SweepDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let report = authed(server, headers, &rid, move |s, c| {
        let grace_s = b.grace_s.unwrap_or(s.sweep_settings().grace_s);
        s.sweep(c, &VaultId(id), grace_s.saturating_mul(1000).min(i64::MAX as u64) as i64)
    })
    .await?;
    Ok(Json(report.into()))
}

/// Every vault's index lag, job queues, review backlog and sizes, and the caches. Server admins only.
#[utoipa::path(get, path = "/api/v1/status", tag = "operations", responses((status = 200, body = memex_api::StatusDto), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody)))]
async fn status<E>(State(server): State<App<E>>, Extension(rid): Extension<RequestId>, headers: HeaderMap) -> Result<Json<memex_api::StatusDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let status = authed(server, headers, &rid, |s, c| s.status(c)).await?;
    Ok(Json(status.into()))
}

/// The status as Prometheus text, for a scraper holding a server admin's token.
#[utoipa::path(get, path = "/api/v1/metrics", tag = "operations", responses((status = 200, content_type = "text/plain", body = String), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody)))]
async fn metrics<E>(State(server): State<App<E>>, Extension(rid): Extension<RequestId>, headers: HeaderMap) -> Result<Response, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let text = authed(server, headers, &rid, |s, c| s.metrics(c)).await?;
    let mut res = text.into_response();
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"));
    Ok(res)
}
