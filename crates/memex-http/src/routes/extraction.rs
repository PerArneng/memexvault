//! Routes for extraction: where a document stands, what generated its results, and running it again.
use super::{App, RequestId, authed, list};
use crate::error::ApiError;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use memex_api::{ErrorBody, ListParams, page_of};
use memex_core::{ExtractionReport, Server, ServerEdges};
use memex_types::{AgentUsage, ExtractionRow, ObjectId, VaultId};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(extraction_status, rerun_extraction, list_derivations),
    components(schemas(
        memex_api::ExtractionStatusDto,
        memex_api::TokenUsageDto,
        memex_api::DerivationDto,
        memex_api::DerivationPage,
        memex_api::RejectedItemDto
    ))
)]
pub(super) struct ExtractionDoc;

pub(super) fn add_routes<E>(api: Router<App<E>>) -> Router<App<E>>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    api.route("/vaults/{id}/sources/{sid}/extraction", get(extraction_status::<E>))
        .route("/vaults/{id}/sources/{sid}/extraction/rerun", post(rerun_extraction::<E>))
        .route("/vaults/{id}/renditions/{rid}/derivations", get(list_derivations::<E>))
}

fn status_dto(r: ExtractionReport) -> memex_api::ExtractionStatusDto {
    let row = r.row;
    let number = |f: fn(&ExtractionRow) -> u32| row.as_ref().map_or(0, f);
    memex_api::ExtractionStatusDto {
        source_id: r.source_id.0,
        rendition_id: r.rendition_id.map(|i| i.0),
        state: r.state.as_str().into(),
        job_id: r.job_id.map(|i| i.0),
        stop_reason: row.as_ref().and_then(|x| x.stop).map(|s| memex_core::extraction::stop_text(s).to_string()),
        segments_total: number(|x| x.segments_total),
        segments_done: number(|x| x.segments_done),
        batches_failed: number(|x| x.batches_failed),
        proposed: number(|x| x.proposed),
        rejected: number(|x| x.rejected),
        tokens: row.as_ref().map_or(AgentUsage::default(), |x| x.usage).into(),
        token_budget: row.as_ref().map_or(0, |x| x.token_budget),
        model: row.as_ref().and_then(|x| x.model.clone()),
        prompt_hash: row.as_ref().and_then(|x| x.prompt_hash.clone()),
        waiting_since_ms: r.waiting_since_ms,
        last_error: r.last_error,
        updated_at_ms: row.as_ref().map(|x| x.updated_at_ms),
    }
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources/{sid}/extraction", tag = "extraction", params(("id" = String, Path), ("sid" = String, Path)), responses((status = 200, body = memex_api::ExtractionStatusDto), (status = 404, body = ErrorBody)))]
async fn extraction_status<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
) -> Result<Json<memex_api::ExtractionStatusDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let report = authed(server, headers, &rid, move |s, c| s.extraction_status(c, &VaultId(id), &ObjectId(sid))).await?;
    Ok(Json(status_dto(report)))
}

/// Queues the document's current rendition again. Segments this prompt already has results for cost nothing and repeat nothing. Asking while a run is open returns that run.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/sources/{sid}/extraction/rerun", tag = "extraction", params(("id" = String, Path), ("sid" = String, Path)), responses((status = 200, body = memex_api::JobDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn rerun_extraction<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
) -> Result<Json<memex_api::JobDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let job = authed(server, headers, &rid, move |s, c| s.rerun_extraction(c, &VaultId(id), &ObjectId(sid))).await?;
    Ok(Json(job.into()))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/renditions/{rid}/derivations", tag = "extraction", params(("id" = String, Path), ("rid" = String, Path), ListParams), responses((status = 200, body = memex_api::DerivationPage), (status = 404, body = ErrorBody)))]
async fn list_derivations<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, r)): Path<(String, String)>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::DerivationPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.list_derivations(c, &VaultId(id), &ObjectId(r), l.cursor.map(ObjectId).as_ref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, l.limit, |x| x.header.id.0.clone());
    Ok(Json(memex_api::DerivationPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}
