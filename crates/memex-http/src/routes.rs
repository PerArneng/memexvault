mod entities;
mod extraction;
mod pages;

use crate::browser::{self, ConsoleLookup};
use crate::error::ApiError;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use memex_api::{
    CreatedTokenDto, ErrorBody, ErrorDetail, GrantDto, HealthDto, ListParams, NameBody, NewPrincipalBody, NewTokenBody, PrincipalDto, PrincipalKindDto,
    PrincipalPage, ScopeDto, TokenDto, TokenPage, VaultDto, VaultPage, VersionDto, WhoAmI, page_of,
};
use memex_core::{Caller, NewToken, Server, ServerEdges, ServerError};
use memex_types::VaultId;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use utoipa::OpenApi;

/// JSON request bodies. Uploads have their own route and limit, in spec 014.
pub const BODY_LIMIT: usize = 1024 * 1024;

#[derive(Clone)]
pub(crate) struct RequestId(pub(crate) String);

pub(crate) type App<E> = Arc<Server<E>>;

#[derive(OpenApi)]
#[openapi(
    info(title = "memexvault", description = "Knowledge base for AI agents"),
    paths(
        health,
        version,
        whoami,
        browser::login,
        browser::logout,
        create_vault,
        list_vaults,
        get_vault,
        rename_vault,
        delete_vault,
        create_principal,
        list_principals,
        disable_principal,
        create_token,
        list_tokens,
        revoke_token,
        propose,
        list_changesets,
        get_changeset,
        inverse_preview,
        approve,
        reject,
        revert,
        list_changes,
        assertions_as_of,
        list_contradictions,
        upload,
        list_sources,
        get_source,
        source_versions,
        upload_limits,
        list_jobs,
        get_job,
        retry_job,
        cancel_job,
        list_renditions,
        reprocess,
        get_rendition,
        list_segments,
        get_segment,
        search,
        search_lexical,
        search_vector,
        list_reanchor_failures,
        list_flagged_pages,
        get_lineage,
        get_highlight,
        get_highlight_png,
        get_file,
        get_pages,
        get_page_image,
        get_thumbnail,
        source_claims,
        segment_claims
    ),
    components(schemas(
        ErrorBody,
        ErrorDetail,
        GrantDto,
        HealthDto,
        NameBody,
        NewPrincipalBody,
        NewTokenBody,
        CreatedTokenDto,
        PrincipalDto,
        PrincipalKindDto,
        PrincipalPage,
        ScopeDto,
        TokenDto,
        TokenPage,
        VaultDto,
        VaultPage,
        VersionDto,
        WhoAmI,
        memex_api::LoginBody,
        memex_api::ProposalBody,
        memex_api::ProposalResult,
        memex_api::ChangeSetDto,
        memex_api::ChangeSetPage,
        memex_api::ReasonBody,
        memex_api::InversePreview,
        memex_api::EventDto,
        memex_api::EventPage,
        memex_api::AssertionList,
        memex_api::ContradictionDto,
        memex_api::ContradictionPage,
        memex_api::SourceDto,
        memex_api::SourcePage,
        memex_api::SourceVersions,
        memex_api::UploadResult,
        memex_api::UploadLimitsDto,
        memex_api::JobDto,
        memex_api::JobPage,
        memex_api::RenditionDto,
        memex_api::RenditionList,
        memex_api::RenditionDetail,
        memex_api::SegmentDto,
        memex_api::LexicalHitDto,
        memex_api::LexicalResultsDto,
        memex_api::SegmentPage,
        memex_api::ReanchorFailureDto,
        memex_api::ReanchorFailurePage,
        memex_api::FlaggedPageDto,
        memex_api::FlaggedPagePage,
        memex_api::RectDto,
        memex_api::EvidenceLineageDto,
        memex_api::LineageDto,
        memex_api::PageFactsDto,
        memex_api::PageListDto,
        memex_api::HighlightDto,
        memex_api::ClaimRefDto,
        memex_api::ClaimRefPage
    ))
)]
struct ApiDoc;

/// The generated contract. The console's types come from this.
pub fn openapi() -> utoipa::openapi::OpenApi {
    let mut doc = ApiDoc::openapi();
    doc.merge(entities::EntityDoc::openapi());
    doc.merge(extraction::ExtractionDoc::openapi());
    doc.merge(pages::PageDoc::openapi());
    doc
}

pub fn router<E>(server: App<E>) -> Router
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    router_with_console(server, browser::embedded_asset)
}

/// `console` finds the console's files by path. Tests pass their own, so they don't depend on what `web/` has built.
pub fn router_with_console<E>(server: App<E>, console: ConsoleLookup) -> Router
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let api = Router::new()
        .route("/health", get(health::<E>))
        .route("/version", get(version))
        .route("/openapi.json", get(|| async { Json(openapi()) }))
        .route("/whoami", get(whoami::<E>))
        .route("/session", post(browser::login::<E>).delete(browser::logout::<E>))
        .route("/vaults", get(list_vaults::<E>).post(create_vault::<E>))
        .route("/vaults/{id}", get(get_vault::<E>).patch(rename_vault::<E>).delete(delete_vault::<E>))
        .route("/vaults/{id}/proposals", post(propose::<E>))
        .route("/vaults/{id}/changesets", get(list_changesets::<E>))
        .route("/vaults/{id}/changesets/{cs}", get(get_changeset::<E>))
        .route("/vaults/{id}/changesets/{cs}/inverse", get(inverse_preview::<E>))
        .route("/vaults/{id}/changesets/{cs}/approve", post(approve::<E>))
        .route("/vaults/{id}/changesets/{cs}/reject", post(reject::<E>))
        .route("/vaults/{id}/changesets/{cs}/revert", post(revert::<E>))
        .route(
            "/vaults/{id}/sources",
            get(list_sources::<E>).post(upload::<E>).layer(DefaultBodyLimit::disable()),
        )
        .route("/vaults/{id}/sources/{sid}", get(get_source::<E>))
        .route("/vaults/{id}/sources/{sid}/versions", get(source_versions::<E>))
        .route("/vaults/{id}/upload-limits", get(upload_limits::<E>))
        .route("/vaults/{id}/sources/{sid}/renditions", get(list_renditions::<E>))
        .route("/vaults/{id}/sources/{sid}/reprocess", post(reprocess::<E>))
        .route("/vaults/{id}/renditions/{rid}", get(get_rendition::<E>))
        .route("/vaults/{id}/renditions/{rid}/segments", get(list_segments::<E>))
        .route("/vaults/{id}/segments/{seg}", get(get_segment::<E>))
        .route("/vaults/{id}/search", get(search::<E>))
        .route("/vaults/{id}/search/lexical", get(search_lexical::<E>))
        .route("/vaults/{id}/search/vector", get(search_vector::<E>))
        .route("/vaults/{id}/reanchor-failures", get(list_reanchor_failures::<E>))
        .route("/vaults/{id}/flagged-pages", get(list_flagged_pages::<E>))
        .route("/vaults/{id}/assertions/{aid}/lineage", get(get_lineage::<E>))
        .route("/vaults/{id}/assertions/{aid}/evidence/{eid}/highlight", get(get_highlight::<E>))
        .route("/vaults/{id}/assertions/{aid}/evidence/{eid}/highlight.png", get(get_highlight_png::<E>))
        .route("/vaults/{id}/sources/{sid}/file", get(get_file::<E>))
        .route("/vaults/{id}/sources/{sid}/pages", get(get_pages::<E>))
        .route("/vaults/{id}/sources/{sid}/pages/{page}/image", get(get_page_image::<E>))
        .route("/vaults/{id}/sources/{sid}/pages/{page}/thumbnail", get(get_thumbnail::<E>))
        .route("/vaults/{id}/sources/{sid}/claims", get(source_claims::<E>))
        .route("/vaults/{id}/segments/{seg}/claims", get(segment_claims::<E>))
        .route("/vaults/{id}/jobs", get(list_jobs::<E>))
        .route("/vaults/{id}/jobs/{jid}", get(get_job::<E>))
        .route("/vaults/{id}/jobs/{jid}/retry", post(retry_job::<E>))
        .route("/vaults/{id}/jobs/{jid}/cancel", post(cancel_job::<E>))
        .route("/vaults/{id}/changes", get(list_changes::<E>))
        .route("/vaults/{id}/assertions/as-of", get(assertions_as_of::<E>))
        .route("/vaults/{id}/contradictions", get(list_contradictions::<E>))
        .route("/principals", get(list_principals::<E>).post(create_principal::<E>))
        .route("/principals/{id}/disable", post(disable_principal::<E>))
        .route("/tokens", get(list_tokens::<E>).post(create_token::<E>))
        .route("/tokens/{id}", axum::routing::delete(revoke_token::<E>));
    let api = entities::add_routes::<E>(api);
    let api = extraction::add_routes::<E>(api);
    let api = pages::add_routes::<E>(api);
    Router::new()
        .nest("/api/v1", api)
        .fallback(move |Extension(rid): Extension<RequestId>, req: Request| browser::console_or_not_found(console, rid, req))
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn(browser::csrf_guard))
        .layer(middleware::from_fn_with_state(server.clone(), request_scope::<E>))
        .with_state(server)
}

/// Gives each request an id and a log line. The line holds the method, the path and the status. Headers and query strings stay out, as they could hold a token.
async fn request_scope<E>(State(server): State<App<E>>, mut req: Request, next: Next) -> Response
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let id = server.new_request_id();
    let (method, path) = (req.method().clone(), req.uri().path().to_string());
    req.extensions_mut().insert(RequestId(id.clone()));
    let mut res = next.run(req).await;
    tracing::info!(request_id = %id, %method, %path, status = res.status().as_u16(), "request");
    browser::security_headers(res.headers_mut());
    if let Ok(v) = HeaderValue::from_str(&id) {
        res.headers_mut().insert("x-request-id", v);
    }
    res
}

async fn method_not_allowed(Extension(rid): Extension<RequestId>) -> ApiError {
    ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "this route doesn't take that method",
        &rid.0,
    )
}

/// Runs a synchronous core call on a blocking thread, after authenticating.
async fn authed<E, T, F>(server: App<E>, headers: HeaderMap, rid: &RequestId, f: F) -> Result<T, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
    T: Send + 'static,
    F: FnOnce(&Server<E>, &Caller) -> Result<T, ServerError> + Send + 'static,
{
    let result = tokio::task::spawn_blocking(move || {
        let caller = browser::caller_from(&server, &headers)?;
        f(&server, &caller)
    })
    .await;
    match result {
        Ok(r) => r.map_err(|e| ApiError::from_server(e, &rid.0)),
        Err(_) => Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "the request failed", &rid.0)),
    }
}

pub(crate) fn body<T: DeserializeOwned>(parsed: Result<Json<T>, JsonRejection>, rid: &RequestId) -> Result<T, ApiError> {
    parsed.map(|Json(v)| v).map_err(|e| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                format!("the body is over the limit of {BODY_LIMIT} bytes"),
                &rid.0,
            )
        } else {
            ApiError::new(StatusCode::BAD_REQUEST, "invalid", e.body_text(), &rid.0)
        }
    })
}

fn list(params: ListParams, rid: &RequestId) -> Result<memex_api::ListRequest, ApiError> {
    params.validated().map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, "invalid", m, &rid.0))
}

#[utoipa::path(get, path = "/api/v1/health", tag = "server", responses((status = 200, body = HealthDto), (status = 503, body = HealthDto)))]
async fn health<E>(State(server): State<App<E>>) -> Response
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    match tokio::task::spawn_blocking(move || server.healthy()).await {
        Ok(true) => (StatusCode::OK, Json(HealthDto { status: "ok".into() })).into_response(),
        _ => (StatusCode::SERVICE_UNAVAILABLE, Json(HealthDto { status: "unavailable".into() })).into_response(),
    }
}

#[utoipa::path(get, path = "/api/v1/version", tag = "server", responses((status = 200, body = VersionDto)))]
async fn version() -> Json<VersionDto> {
    Json(VersionDto {
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

#[utoipa::path(get, path = "/api/v1/whoami", tag = "server", responses((status = 200, body = WhoAmI), (status = 401, body = ErrorBody)))]
async fn whoami<E>(State(server): State<App<E>>, Extension(rid): Extension<RequestId>, headers: HeaderMap) -> Result<Json<WhoAmI>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    authed(server, headers, &rid, |_, c| {
        Ok(WhoAmI {
            principal: c.principal.clone().into(),
            token_id: c.token_id.clone(),
            grants: c.grants.iter().cloned().map(Into::into).collect(),
        })
    })
    .await
    .map(Json)
}

#[utoipa::path(post, path = "/api/v1/vaults", tag = "vaults", request_body = NameBody, responses((status = 201, body = VaultDto), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn create_vault<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    parsed: Result<Json<NameBody>, JsonRejection>,
) -> Result<(StatusCode, Json<VaultDto>), ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let v = authed(server, headers, &rid, move |s, c| s.create_vault(c, &b.name)).await?;
    Ok((StatusCode::CREATED, Json(v.into())))
}

#[utoipa::path(get, path = "/api/v1/vaults", tag = "vaults", params(ListParams), responses((status = 200, body = VaultPage)))]
async fn list_vaults<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> Result<Json<VaultPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| s.list_vaults(c, l.cursor.map(VaultId).as_ref(), l.limit + 1)).await?;
    let (items, next_cursor) = page_of(rows, l.limit, |v| v.id.0.clone());
    Ok(Json(VaultPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}", tag = "vaults", params(("id" = String, Path)), responses((status = 200, body = VaultDto), (status = 404, body = ErrorBody)))]
async fn get_vault<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<VaultDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let v = authed(server, headers, &rid, move |s, c| s.get_vault(c, &VaultId(id))).await?;
    Ok(Json(v.into()))
}

#[utoipa::path(patch, path = "/api/v1/vaults/{id}", tag = "vaults", params(("id" = String, Path)), request_body = NameBody, responses((status = 200, body = VaultDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn rename_vault<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    parsed: Result<Json<NameBody>, JsonRejection>,
) -> Result<Json<VaultDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let v = authed(server, headers, &rid, move |s, c| s.rename_vault(c, &VaultId(id), &b.name)).await?;
    Ok(Json(v.into()))
}

#[utoipa::path(delete, path = "/api/v1/vaults/{id}", tag = "vaults", params(("id" = String, Path)), responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn delete_vault<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    authed(server, headers, &rid, move |s, c| s.delete_vault(c, &VaultId(id))).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/principals", tag = "principals", request_body = NewPrincipalBody, responses((status = 201, body = PrincipalDto), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn create_principal<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    parsed: Result<Json<NewPrincipalBody>, JsonRejection>,
) -> Result<(StatusCode, Json<PrincipalDto>), ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let p = authed(server, headers, &rid, move |s, c| s.create_principal(c, &b.name, b.kind.into(), b.server_admin)).await?;
    Ok((StatusCode::CREATED, Json(p.into())))
}

#[utoipa::path(get, path = "/api/v1/principals", tag = "principals", params(ListParams), responses((status = 200, body = PrincipalPage), (status = 403, body = ErrorBody)))]
async fn list_principals<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> Result<Json<PrincipalPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| s.list_principals(c, l.cursor.as_deref(), l.limit + 1)).await?;
    let (items, next_cursor) = page_of(rows, l.limit, |p| p.id.clone());
    Ok(Json(PrincipalPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(post, path = "/api/v1/principals/{id}/disable", tag = "principals", params(("id" = String, Path)), responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn disable_principal<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    authed(server, headers, &rid, move |s, c| s.disable_principal(c, &id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/tokens", tag = "tokens", request_body = NewTokenBody, responses((status = 201, body = CreatedTokenDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn create_token<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    parsed: Result<Json<NewTokenBody>, JsonRejection>,
) -> Result<(StatusCode, Json<CreatedTokenDto>), ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let new = NewToken {
        principal_id: b.principal_id,
        name: b.name,
        grants: b.grants.into_iter().map(Into::into).collect(),
        expires_at_ms: b.expires_at_ms,
    };
    let created = authed(server, headers, &rid, move |s, c| s.create_token(c, new)).await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedTokenDto {
            token: created.token,
            details: created.row.into(),
        }),
    ))
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct TokenListExtra {
    /// Every principal's tokens, for server admins. The default is your own.
    all: Option<bool>,
}

#[utoipa::path(get, path = "/api/v1/tokens", tag = "tokens", params(ListParams, TokenListExtra), responses((status = 200, body = TokenPage), (status = 403, body = ErrorBody)))]
async fn list_tokens<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
    Query(extra): Query<TokenListExtra>,
) -> Result<Json<TokenPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.list_tokens(c, extra.all.unwrap_or(false), l.cursor.as_deref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, l.limit, |t| t.id.clone());
    Ok(Json(TokenPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(delete, path = "/api/v1/tokens/{id}", tag = "tokens", params(("id" = String, Path)), responses((status = 204), (status = 404, body = ErrorBody)))]
async fn revoke_token<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    authed(server, headers, &rid, move |s, c| s.revoke_token(c, &id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn changeset_state(text: Option<&str>, rid: &RequestId) -> Result<Option<memex_types::ChangeSetState>, ApiError> {
    use memex_types::ChangeSetState as S;
    match text {
        None => Ok(None),
        Some("pending") => Ok(Some(S::Pending)),
        Some("committed") => Ok(Some(S::Committed)),
        Some("rejected") => Ok(Some(S::Rejected)),
        Some(other) => Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid", format!("unknown state '{other}'"), &rid.0)),
    }
}

#[utoipa::path(post, path = "/api/v1/vaults/{id}/proposals", tag = "governance", params(("id" = String, Path), memex_api::ProposalQuery), request_body = memex_api::ProposalBody, responses((status = 200, body = memex_api::ProposalResult), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn propose<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<memex_api::ProposalQuery>,
    parsed: Result<Json<memex_api::ProposalBody>, JsonRejection>,
) -> Result<Json<memex_api::ProposalResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let dry = q.dry_run.unwrap_or(false);
    let p = memex_core::Proposal {
        ops: b.ops,
        rationale: b.rationale,
        idempotency_key: b.idempotency_key,
    };
    let out = authed(server, headers, &rid, move |s, c| s.propose(c, &VaultId(id), p, dry)).await?;
    Ok(Json(memex_api::ProposalResult {
        changeset: out.changeset.map(Into::into),
        validators: out.validators.iter().filter_map(|v| serde_json::to_value(v).ok()).collect(),
        risk: serde_json::to_value(out.risk)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default(),
        contradictions: out
            .contradictions
            .into_iter()
            .map(|(a, b)| memex_api::ContradictionDto { claim: a.0, other: b.0 })
            .collect(),
        replayed: out.replayed,
        dry_run: out.dry_run,
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/changesets", tag = "governance", params(("id" = String, Path), ListParams, memex_api::StateFilter), responses((status = 200, body = memex_api::ChangeSetPage), (status = 404, body = ErrorBody)))]
async fn list_changesets<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
    Query(filter): Query<memex_api::StateFilter>,
) -> Result<Json<memex_api::ChangeSetPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let state = changeset_state(filter.state.as_deref(), &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.list_changesets(c, &VaultId(id), state, l.cursor.map(memex_types::ObjectId).as_ref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, l.limit, |c| c.header.id.0.clone());
    Ok(Json(memex_api::ChangeSetPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/changesets/{cs}", tag = "governance", params(("id" = String, Path), ("cs" = String, Path)), responses((status = 200, body = memex_api::ChangeSetDto), (status = 404, body = ErrorBody)))]
async fn get_changeset<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, cs)): Path<(String, String)>,
) -> Result<Json<memex_api::ChangeSetDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let row = authed(server, headers, &rid, move |s, c| s.get_changeset(c, &VaultId(id), &memex_types::ObjectId(cs))).await?;
    Ok(Json(row.into()))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/changesets/{cs}/inverse", tag = "governance", params(("id" = String, Path), ("cs" = String, Path)), responses((status = 200, body = memex_api::InversePreview), (status = 404, body = ErrorBody)))]
async fn inverse_preview<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, cs)): Path<(String, String)>,
) -> Result<Json<memex_api::InversePreview>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let (mutations, blocked) = authed(server, headers, &rid, move |s, c| {
        s.inverse_preview(c, &VaultId(id), &memex_types::ObjectId(cs))
    })
    .await?;
    Ok(Json(memex_api::InversePreview {
        mutations: mutations.iter().filter_map(|m| serde_json::to_value(m).ok()).collect(),
        reversible: blocked.is_none(),
        blocked_because: blocked,
    }))
}

#[utoipa::path(post, path = "/api/v1/vaults/{id}/changesets/{cs}/approve", tag = "governance", params(("id" = String, Path), ("cs" = String, Path)), request_body = memex_api::ReasonBody, responses((status = 200, body = memex_api::ChangeSetDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn approve<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, cs)): Path<(String, String)>,
    parsed: Result<Json<memex_api::ReasonBody>, JsonRejection>,
) -> Result<Json<memex_api::ChangeSetDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let row = authed(server, headers, &rid, move |s, c| {
        s.approve(c, &VaultId(id), &memex_types::ObjectId(cs), b.reason)
    })
    .await?;
    Ok(Json(row.into()))
}

#[utoipa::path(post, path = "/api/v1/vaults/{id}/changesets/{cs}/reject", tag = "governance", params(("id" = String, Path), ("cs" = String, Path)), request_body = memex_api::ReasonBody, responses((status = 200, body = memex_api::ChangeSetDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn reject<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, cs)): Path<(String, String)>,
    parsed: Result<Json<memex_api::ReasonBody>, JsonRejection>,
) -> Result<Json<memex_api::ChangeSetDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let row = authed(server, headers, &rid, move |s, c| {
        s.reject(c, &VaultId(id), &memex_types::ObjectId(cs), b.reason.unwrap_or_default())
    })
    .await?;
    Ok(Json(row.into()))
}

#[utoipa::path(post, path = "/api/v1/vaults/{id}/changesets/{cs}/revert", tag = "governance", params(("id" = String, Path), ("cs" = String, Path)), request_body = memex_api::ReasonBody, responses((status = 200, body = memex_api::ChangeSetDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn revert<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, cs)): Path<(String, String)>,
    parsed: Result<Json<memex_api::ReasonBody>, JsonRejection>,
) -> Result<Json<memex_api::ChangeSetDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let row = authed(server, headers, &rid, move |s, c| {
        s.revert(c, &VaultId(id), &memex_types::ObjectId(cs), b.reason.unwrap_or_default())
    })
    .await?;
    Ok(Json(row.into()))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/changes", tag = "governance", params(("id" = String, Path), ListParams), responses((status = 200, body = memex_api::EventPage), (status = 404, body = ErrorBody)))]
async fn list_changes<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::EventPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let after = match &l.cursor {
        None => None,
        Some(c) => Some(
            c.parse::<u64>()
                .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "invalid", "the cursor isn't one this route gave out", &rid.0))?,
        ),
    };
    let rows = authed(server, headers, &rid, move |s, c| s.changes_since(c, &VaultId(id), after)).await?;
    let (items, next_cursor) = page_of(rows, l.limit, |e| e.seq.to_string());
    Ok(Json(memex_api::EventPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/assertions/as-of", tag = "governance", params(("id" = String, Path), memex_api::AsOfQuery), responses((status = 200, body = memex_api::AssertionList), (status = 404, body = ErrorBody)))]
async fn assertions_as_of<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<memex_api::AsOfQuery>,
) -> Result<Json<memex_api::AssertionList>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let rows = authed(server, headers, &rid, move |s, c| s.believed_at(c, &VaultId(id), &q.subject, q.seq)).await?;
    Ok(Json(memex_api::AssertionList::of(&rows)))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/contradictions", tag = "governance", params(("id" = String, Path), ListParams), responses((status = 200, body = memex_api::ContradictionPage), (status = 404, body = ErrorBody)))]
async fn list_contradictions<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::ContradictionPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let pairs = authed(server, headers, &rid, move |s, c| s.open_contradictions(c, &VaultId(id))).await?;
    let rest: Vec<memex_api::ContradictionDto> = pairs
        .into_iter()
        .map(|(a, b)| memex_api::ContradictionDto { claim: a.0, other: b.0 })
        .filter(|d| l.cursor.as_ref().is_none_or(|c| &d.claim > c))
        .collect();
    let (items, next_cursor) = page_of(rest, l.limit, |d| d.claim.clone());
    Ok(Json(memex_api::ContradictionPage { items, next_cursor }))
}

/// Hands a request body's chunks to the blocking side. The channel is bounded, so a slow disk slows the client and memory stays flat.
struct BodyReader {
    rx: tokio::sync::mpsc::Receiver<Result<axum::body::Bytes, std::io::Error>>,
    current: axum::body::Bytes,
}

impl std::io::Read for BodyReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.current.is_empty() {
            match self.rx.blocking_recv() {
                Some(Ok(chunk)) => self.current = chunk,
                Some(Err(e)) => return Err(e),
                None => return Ok(0),
            }
        }
        let n = buf.len().min(self.current.len());
        buf[..n].copy_from_slice(&self.current.split_to(n));
        Ok(n)
    }
}

/// The upload limit. A setting can replace this when one is needed.
fn limits() -> memex_core::UploadLimits {
    memex_core::UploadLimits::default()
}

#[utoipa::path(post, path = "/api/v1/vaults/{id}/sources", tag = "sources", params(("id" = String, Path), memex_api::UploadQuery), request_body(content = Vec<u8>, content_type = "application/octet-stream"), responses((status = 201, body = memex_api::UploadResult), (status = 200, body = memex_api::UploadResult), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 413, body = ErrorBody), (status = 415, body = ErrorBody), (status = 429, body = ErrorBody)))]
async fn upload<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<memex_api::UploadQuery>,
    req_body: axum::body::Body,
) -> Result<Response, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    use futures_util::StreamExt;
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(async move {
        let mut stream = req_body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            if tx.send(chunk.map_err(std::io::Error::other)).await.is_err() {
                break;
            }
        }
    });
    let request = memex_core::UploadRequest {
        name: q.name,
        trust_tier: q.trust_tier.unwrap_or_else(|| "unverified".into()),
        metadata: serde_json::json!({}),
        replaces: q.replaces.map(memex_types::ObjectId),
    };
    let out = authed(server, headers, &rid, move |s, c| {
        let mut reader = BodyReader {
            rx,
            current: axum::body::Bytes::new(),
        };
        s.upload(c, &VaultId(id), request, limits(), &mut reader)
    })
    .await?;
    let status = if out.existing { StatusCode::OK } else { StatusCode::CREATED };
    let job_id = out.job.map(|j| j.header.id.0);
    Ok((
        status,
        Json(memex_api::UploadResult {
            source: out.source.into(),
            job_id,
            existing: out.existing,
        }),
    )
        .into_response())
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/upload-limits", tag = "sources", params(("id" = String, Path)), responses((status = 200, body = memex_api::UploadLimitsDto), (status = 404, body = ErrorBody)))]
async fn upload_limits<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<memex_api::UploadLimitsDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    authed(server, headers, &rid, move |s, c| s.get_vault(c, &VaultId(id))).await?;
    Ok(Json(memex_api::UploadLimitsDto {
        max_bytes: limits().max_bytes,
        allowed_media_types: memex_core::ALLOWED_MEDIA_TYPES.iter().map(|t| t.to_string()).collect(),
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources", tag = "sources", params(("id" = String, Path), ListParams), responses((status = 200, body = memex_api::SourcePage), (status = 404, body = ErrorBody)))]
async fn list_sources<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::SourcePage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.list_sources(c, &VaultId(id), l.cursor.map(memex_types::ObjectId).as_ref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, l.limit, |s| s.header.id.0.clone());
    Ok(Json(memex_api::SourcePage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources/{sid}", tag = "sources", params(("id" = String, Path), ("sid" = String, Path)), responses((status = 200, body = memex_api::SourceDto), (status = 404, body = ErrorBody)))]
async fn get_source<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
) -> Result<Json<memex_api::SourceDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let s = authed(server, headers, &rid, move |s, c| s.get_source(c, &VaultId(id), &memex_types::ObjectId(sid))).await?;
    Ok(Json(s.into()))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources/{sid}/versions", tag = "sources", params(("id" = String, Path), ("sid" = String, Path)), responses((status = 200, body = memex_api::SourceVersions), (status = 404, body = ErrorBody)))]
async fn source_versions<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
) -> Result<Json<memex_api::SourceVersions>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let rows = authed(server, headers, &rid, move |s, c| {
        s.source_versions(c, &VaultId(id), &memex_types::ObjectId(sid))
    })
    .await?;
    Ok(Json(memex_api::SourceVersions {
        items: rows.into_iter().map(Into::into).collect(),
    }))
}

fn job_state(text: Option<&str>, rid: &RequestId) -> Result<Option<memex_types::JobState>, ApiError> {
    use memex_types::JobState as S;
    match text {
        None => Ok(None),
        Some("queued") => Ok(Some(S::Queued)),
        Some("running") => Ok(Some(S::Running)),
        Some("done") => Ok(Some(S::Done)),
        Some("dead") => Ok(Some(S::Dead)),
        Some("cancelled") => Ok(Some(S::Cancelled)),
        Some(other) => Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid", format!("unknown state '{other}'"), &rid.0)),
    }
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/jobs", tag = "jobs", params(("id" = String, Path), ListParams, memex_api::JobFilter), responses((status = 200, body = memex_api::JobPage), (status = 404, body = ErrorBody)))]
async fn list_jobs<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
    Query(filter): Query<memex_api::JobFilter>,
) -> Result<Json<memex_api::JobPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let state = job_state(filter.state.as_deref(), &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.list_jobs(
            c,
            &VaultId(id),
            filter.kind.as_deref(),
            state,
            l.cursor.map(memex_types::ObjectId).as_ref(),
            l.limit + 1,
        )
    })
    .await?;
    let (items, next_cursor) = page_of(rows, l.limit, |j| j.header.id.0.clone());
    Ok(Json(memex_api::JobPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/jobs/{jid}", tag = "jobs", params(("id" = String, Path), ("jid" = String, Path)), responses((status = 200, body = memex_api::JobDto), (status = 404, body = ErrorBody)))]
async fn get_job<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, jid)): Path<(String, String)>,
) -> Result<Json<memex_api::JobDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let j = authed(server, headers, &rid, move |s, c| s.get_job(c, &VaultId(id), &memex_types::ObjectId(jid))).await?;
    Ok(Json(j.into()))
}

#[utoipa::path(post, path = "/api/v1/vaults/{id}/jobs/{jid}/retry", tag = "jobs", params(("id" = String, Path), ("jid" = String, Path)), responses((status = 200, body = memex_api::JobDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn retry_job<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, jid)): Path<(String, String)>,
) -> Result<Json<memex_api::JobDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let j = authed(server, headers, &rid, move |s, c| s.retry_job(c, &VaultId(id), &memex_types::ObjectId(jid))).await?;
    Ok(Json(j.into()))
}

#[utoipa::path(post, path = "/api/v1/vaults/{id}/jobs/{jid}/cancel", tag = "jobs", params(("id" = String, Path), ("jid" = String, Path)), responses((status = 200, body = memex_api::JobDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn cancel_job<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, jid)): Path<(String, String)>,
) -> Result<Json<memex_api::JobDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let j = authed(server, headers, &rid, move |s, c| s.cancel_job(c, &VaultId(id), &memex_types::ObjectId(jid))).await?;
    Ok(Json(j.into()))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources/{sid}/renditions", tag = "renditions", params(("id" = String, Path), ("sid" = String, Path)), responses((status = 200, body = memex_api::RenditionList), (status = 404, body = ErrorBody)))]
async fn list_renditions<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
) -> Result<Json<memex_api::RenditionList>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let rows = authed(server, headers, &rid, move |s, c| {
        s.list_renditions(c, &VaultId(id), &memex_types::ObjectId(sid))
    })
    .await?;
    Ok(Json(memex_api::RenditionList {
        items: rows.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/vaults/{id}/sources/{sid}/reprocess", tag = "renditions", params(("id" = String, Path), ("sid" = String, Path)), responses((status = 200, body = memex_api::JobDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn reprocess<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
) -> Result<Json<memex_api::JobDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let j = authed(server, headers, &rid, move |s, c| {
        s.reprocess_source(c, &VaultId(id), &memex_types::ObjectId(sid))
    })
    .await?;
    Ok(Json(j.into()))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/renditions/{rid}", tag = "renditions", params(("id" = String, Path), ("rid" = String, Path)), responses((status = 200, body = memex_api::RenditionDetail), (status = 404, body = ErrorBody)))]
async fn get_rendition<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, r)): Path<(String, String)>,
) -> Result<Json<memex_api::RenditionDetail>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let detail = authed(server, headers, &rid, move |s, c| {
        let (vault, rendition) = (VaultId(id), memex_types::ObjectId(r));
        let row = s.get_rendition(c, &vault, &rendition)?;
        let text = s.rendition_blob(c, &vault, &rendition, false)?;
        let table = s.rendition_blob(c, &vault, &rendition, true)?;
        Ok(memex_api::RenditionDetail {
            rendition: row.into(),
            canonical_text: String::from_utf8_lossy(&text).into_owned(),
            word_table: serde_json::from_slice(&table).unwrap_or(serde_json::Value::Null),
        })
    })
    .await?;
    Ok(Json(detail))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/renditions/{rid}/segments", tag = "renditions", params(("id" = String, Path), ("rid" = String, Path), ListParams), responses((status = 200, body = memex_api::SegmentPage), (status = 404, body = ErrorBody)))]
async fn list_segments<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, r)): Path<(String, String)>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::SegmentPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.list_segments(
            c,
            &VaultId(id),
            &memex_types::ObjectId(r),
            l.cursor.map(memex_types::ObjectId).as_ref(),
            l.limit + 1,
        )
    })
    .await?;
    let (items, next_cursor) = page_of(rows, l.limit, |x| x.header.id.0.clone());
    Ok(Json(memex_api::SegmentPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/segments/{seg}", tag = "renditions", params(("id" = String, Path), ("seg" = String, Path)), responses((status = 200, body = memex_api::SegmentDto), (status = 404, body = ErrorBody)))]
async fn get_segment<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, seg)): Path<(String, String)>,
) -> Result<Json<memex_api::SegmentDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let row = authed(server, headers, &rid, move |s, c| s.get_segment(c, &VaultId(id), &memex_types::ObjectId(seg))).await?;
    Ok(Json(row.into()))
}

/// How often `min_seq` checks again while it waits.
const MIN_SEQ_POLL: std::time::Duration = std::time::Duration::from_millis(50);

#[utoipa::path(get, path = "/api/v1/vaults/{id}/search", tag = "search", params(("id" = String, Path), memex_api::SearchParams), responses((status = 200, body = memex_api::EvidencePackageDto), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody), (status = 429, body = ErrorBody), (status = 503, body = ErrorBody)))]
async fn search<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<memex_api::SearchParams>,
) -> Result<Json<memex_api::EvidencePackageDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let (request, wait_ms) = params.validated().map_err(|m| ApiError::from_server(ServerError::Invalid(m), &rid.0))?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(u64::from(wait_ms));
    loop {
        let (r, v) = (request.clone(), VaultId(id.clone()));
        match authed(server.clone(), headers.clone(), &rid, move |s, c| s.search(c, &v, &r)).await {
            Ok(package) => return Ok(Json(package.into())),
            // An index catches up on its own, so this is the one failure worth waiting out.
            Err(e) if e.is_not_caught_up() && tokio::time::Instant::now() + MIN_SEQ_POLL < deadline => tokio::time::sleep(MIN_SEQ_POLL).await,
            Err(e) => return Err(e),
        }
    }
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/search/lexical", tag = "search", params(("id" = String, Path), memex_api::LexicalSearchParams), responses((status = 200, body = memex_api::LexicalResultsDto), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody), (status = 503, body = ErrorBody)))]
async fn search_lexical<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<memex_api::LexicalSearchParams>,
) -> Result<Json<memex_api::LexicalResultsDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let (text, filter, limit) = params.validated().map_err(|m| ApiError::from_server(ServerError::Invalid(m), &rid.0))?;
    let results = authed(server, headers, &rid, move |s, c| s.search_lexical(c, &VaultId(id), &text, filter, limit)).await?;
    Ok(Json(results.into()))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/search/vector", tag = "search", params(("id" = String, Path), memex_api::LexicalSearchParams), responses((status = 200, body = memex_api::LexicalResultsDto), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody), (status = 503, body = ErrorBody)))]
async fn search_vector<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<memex_api::LexicalSearchParams>,
) -> Result<Json<memex_api::LexicalResultsDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let (text, filter, limit) = params.validated().map_err(|m| ApiError::from_server(ServerError::Invalid(m), &rid.0))?;
    let results = authed(server, headers, &rid, move |s, c| s.search_vector(c, &VaultId(id), &text, filter, limit)).await?;
    Ok(Json(results.into()))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/reanchor-failures", tag = "renditions", params(("id" = String, Path), ListParams), responses((status = 200, body = memex_api::ReanchorFailurePage), (status = 404, body = ErrorBody)))]
async fn list_reanchor_failures<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::ReanchorFailurePage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.list_broken_anchors(c, &VaultId(id), l.cursor.map(memex_types::ObjectId).as_ref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, l.limit, |e| e.header.id.0.clone());
    Ok(Json(memex_api::ReanchorFailurePage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/flagged-pages", tag = "renditions", params(("id" = String, Path), ListParams), responses((status = 200, body = memex_api::FlaggedPagePage), (status = 404, body = ErrorBody)))]
async fn list_flagged_pages<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::FlaggedPagePage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| s.flagged_pages(c, &VaultId(id))).await?;
    let key = |r: &memex_types::RenditionRow, page: u32| format!("{}:{page:08}", r.header.id.0);
    let rest: Vec<_> = rows
        .into_iter()
        .filter(|(r, o)| l.cursor.as_ref().is_none_or(|c| &key(r, o.index) > c))
        .collect();
    let (items, next_cursor) = page_of(rest, l.limit, |(r, o)| key(r, o.index));
    let items = items
        .into_iter()
        .map(|(r, o)| memex_api::FlaggedPageDto {
            source_id: r.source_id.0,
            rendition_id: r.header.id.0,
            page: o.index,
            outcome: serde_json::to_value(o).unwrap_or(serde_json::Value::Null),
        })
        .collect();
    Ok(Json(memex_api::FlaggedPagePage { items, next_cursor }))
}

fn rects_of(a: Option<&memex_types::Anchor>) -> Vec<memex_api::RectDto> {
    a.map_or(vec![], |a| {
        a.rects
            .iter()
            .map(|(page, r)| memex_api::RectDto {
                page: *page,
                x0: r.x0,
                y0: r.y0,
                x1: r.x1,
                y1: r.y1,
            })
            .collect()
    })
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/assertions/{aid}/lineage", tag = "provenance", params(("id" = String, Path), ("aid" = String, Path)), responses((status = 200, body = memex_api::LineageDto), (status = 404, body = ErrorBody)))]
async fn get_lineage<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, aid)): Path<(String, String)>,
) -> Result<Json<memex_api::LineageDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = authed(server, headers, &rid, move |s, c| s.lineage(c, &VaultId(id), &memex_types::ObjectId(aid))).await?;
    let text = |v: serde_json::Value| v.as_str().map(str::to_string).unwrap_or_default();
    Ok(Json(memex_api::LineageDto {
        changeset_id: l.assertion.changeset_id.0.clone(),
        assertion: serde_json::to_value(&l.assertion).unwrap_or(serde_json::Value::Null),
        proposer: l.changeset.as_ref().map(|c| c.proposer.clone()),
        proposer_is_machine: l.changeset.as_ref().map(|c| c.proposer_is_machine),
        decided_by: l.changeset.as_ref().and_then(|c| c.decided_by.clone()),
        rationale: l.changeset.as_ref().map(|c| c.rationale.clone()),
        evidence: l
            .evidence
            .into_iter()
            .map(|e| memex_api::EvidenceLineageDto {
                rects: rects_of(e.anchor.as_ref()),
                evidence_id: e.evidence.header.id.0,
                quote: e.evidence.quote,
                stance: text(serde_json::to_value(e.evidence.stance).unwrap_or(serde_json::Value::Null)),
                segment_id: e.segment.header.id.0,
                rendition_id: e.rendition.header.id.0,
                parser: e.rendition.parser,
                source_id: e.source.header.id.0,
                source_name: e.source.name,
                source_sha256: e.source.sha256,
                trust_tier: e.source.trust_tier,
                superseded: e.superseded,
                broken: e.broken,
            })
            .collect(),
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/assertions/{aid}/evidence/{eid}/highlight", tag = "provenance", params(("id" = String, Path), ("aid" = String, Path), ("eid" = String, Path)), responses((status = 200, body = memex_api::HighlightDto), (status = 404, body = ErrorBody)))]
async fn get_highlight<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, aid, eid)): Path<(String, String, String)>,
) -> Result<Json<memex_api::HighlightDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let dto = authed(server, headers, &rid, move |s, c| {
        let vault = VaultId(id);
        let e = s.evidence_region(c, &vault, &memex_types::ObjectId(aid), &memex_types::ObjectId(eid))?;
        let page = e.anchor.as_ref().and_then(|a| a.rects.first().map(|r| r.0)).unwrap_or(0);
        let facts = s
            .list_pages(c, &vault, &e.source.header.id)
            .ok()
            .and_then(|l| l.pages.into_iter().find(|p| p.index == page));
        Ok(memex_api::HighlightDto {
            source_id: e.source.header.id.0,
            page,
            page_width: facts.as_ref().map_or(0, |f| f.width),
            page_height: facts.as_ref().map_or(0, |f| f.height),
            broken: e.broken,
            rects: rects_of(e.anchor.as_ref()),
        })
    })
    .await?;
    Ok(Json(dto))
}

fn png_response(image: memex_core::PageImage) -> Response {
    (
        [(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "private, max-age=3600")],
        image.png,
    )
        .into_response()
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/assertions/{aid}/evidence/{eid}/highlight.png", tag = "provenance", params(("id" = String, Path), ("aid" = String, Path), ("eid" = String, Path), memex_api::ImageQuery), responses((status = 200, description = "A PNG of the page with the region highlighted", content_type = "image/png"), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn get_highlight_png<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, aid, eid)): Path<(String, String, String)>,
    Query(q): Query<memex_api::ImageQuery>,
) -> Result<Response, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let image = authed(server, headers, &rid, move |s, c| {
        let vault = VaultId(id);
        let e = s.evidence_region(c, &vault, &memex_types::ObjectId(aid), &memex_types::ObjectId(eid))?;
        let anchor = e
            .anchor
            .as_ref()
            .ok_or_else(|| ServerError::Conflict("this anchor is broken, so there's nothing to highlight".into()))?;
        let page = anchor.rects.first().map_or(0, |r| r.0);
        let rects: Vec<memex_types::Rect> = anchor.rects.iter().filter(|r| r.0 == page).map(|r| r.1).collect();
        s.page_image(c, &vault, &e.source.header.id, page, q.dpi.unwrap_or(150), &rects)
    })
    .await?;
    Ok(png_response(image))
}

/// `bytes=a-b`, `bytes=a-` or `bytes=-n`, for a resource of this size. None for anything else, which then gets the whole file. Pure.
pub fn parse_range(value: &str, size: u64) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?;
    if spec.contains(',') || size == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        return if n == 0 { None } else { Some((size.saturating_sub(n), size - 1)) };
    }
    let start: u64 = a.parse().ok()?;
    let end: u64 = if b.is_empty() { size - 1 } else { b.parse::<u64>().ok()?.min(size - 1) };
    if start > end { None } else { Some((start, end)) }
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources/{sid}/file", tag = "provenance", params(("id" = String, Path), ("sid" = String, Path)), responses((status = 200, description = "The original file, as an attachment"), (status = 206, description = "Part of it, for a Range request"), (status = 404, body = ErrorBody)))]
async fn get_file<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
) -> Result<Response, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let out = authed(server, headers, &rid, move |s, c| {
        let (vault, source) = (VaultId(id), memex_types::ObjectId(sid));
        let row = s.get_source(c, &vault, &source)?;
        let part = range.as_deref().and_then(|r| parse_range(r, row.size));
        let (offset, len) = part.map_or((0, None), |(a, b)| (a, Some(b - a + 1)));
        let (_, bytes) = s.read_original(c, &vault, &source, offset, len)?;
        Ok((row, part, bytes))
    })
    .await?;
    let (row, part, bytes) = out;
    // An attachment in a sandbox, so an uploaded HTML or SVG file can't run on our origin, even if a browser opens it anyway.
    let safe_name: String = row
        .name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    let disposition = format!("attachment; filename=\"{safe_name}\"");
    let mut res = (StatusCode::OK, bytes).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(browser::DOWNLOAD_CSP));
    if let Ok(v) = HeaderValue::from_str(&disposition) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    if let Some((a, b)) = part {
        if let Ok(v) = HeaderValue::from_str(&format!("bytes {a}-{b}/{}", row.size)) {
            h.insert(header::CONTENT_RANGE, v);
        }
        *res.status_mut() = StatusCode::PARTIAL_CONTENT;
    }
    Ok(res)
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources/{sid}/pages", tag = "provenance", params(("id" = String, Path), ("sid" = String, Path)), responses((status = 200, body = memex_api::PageListDto), (status = 404, body = ErrorBody)))]
async fn get_pages<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
) -> Result<Json<memex_api::PageListDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let list = authed(server, headers, &rid, move |s, c| s.list_pages(c, &VaultId(id), &memex_types::ObjectId(sid))).await?;
    Ok(Json(memex_api::PageListDto {
        rendition_id: list.rendition_id.0,
        page_count: list.pages.len() as u32,
        allowed_dpi: memex_core::ALLOWED_DPI.to_vec(),
        pages: list
            .pages
            .into_iter()
            .map(|p| memex_api::PageFactsDto {
                index: p.index,
                width: p.width,
                height: p.height,
                rotation: p.rotation,
            })
            .collect(),
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources/{sid}/pages/{page}/image", tag = "provenance", params(("id" = String, Path), ("sid" = String, Path), ("page" = u32, Path), memex_api::ImageQuery), responses((status = 200, description = "A PNG of the page", content_type = "image/png"), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn get_page_image<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid, page)): Path<(String, String, u32)>,
    Query(q): Query<memex_api::ImageQuery>,
) -> Result<Response, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let image = authed(server, headers, &rid, move |s, c| {
        s.page_image(c, &VaultId(id), &memex_types::ObjectId(sid), page, q.dpi.unwrap_or(150), &[])
    })
    .await?;
    Ok(png_response(image))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources/{sid}/pages/{page}/thumbnail", tag = "provenance", params(("id" = String, Path), ("sid" = String, Path), ("page" = u32, Path)), responses((status = 200, description = "A small PNG of the page", content_type = "image/png"), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn get_thumbnail<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid, page)): Path<(String, String, u32)>,
) -> Result<Response, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let image = authed(server, headers, &rid, move |s, c| {
        s.thumbnail(c, &VaultId(id), &memex_types::ObjectId(sid), page)
    })
    .await?;
    Ok(png_response(image))
}

fn claim_page(rows: Vec<memex_core::ClaimRef>, l: &memex_api::ListRequest) -> memex_api::ClaimRefPage {
    let rest: Vec<_> = rows.into_iter().filter(|r| l.cursor.as_ref().is_none_or(|c| &r.evidence_id.0 > c)).collect();
    let (items, next_cursor) = page_of(rest, l.limit, |r| r.evidence_id.0.clone());
    memex_api::ClaimRefPage {
        items: items
            .into_iter()
            .map(|r| memex_api::ClaimRefDto {
                assertion_id: r.assertion.header.id.0,
                evidence_id: r.evidence_id.0,
                subject: r.assertion.subject,
                predicate: r.assertion.predicate,
                value: r.assertion.value,
            })
            .collect(),
        next_cursor,
    }
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources/{sid}/claims", tag = "provenance", params(("id" = String, Path), ("sid" = String, Path), ListParams), responses((status = 200, body = memex_api::ClaimRefPage), (status = 404, body = ErrorBody)))]
async fn source_claims<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::ClaimRefPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.claims_citing_source(c, &VaultId(id), &memex_types::ObjectId(sid))
    })
    .await?;
    Ok(Json(claim_page(rows, &l)))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/segments/{seg}/claims", tag = "provenance", params(("id" = String, Path), ("seg" = String, Path), ListParams), responses((status = 200, body = memex_api::ClaimRefPage), (status = 404, body = ErrorBody)))]
async fn segment_claims<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, seg)): Path<(String, String)>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::ClaimRefPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.claims_citing_segment(c, &VaultId(id), &memex_types::ObjectId(seg))
    })
    .await?;
    Ok(Json(claim_page(rows, &l)))
}

/// Serves until the shutdown future finishes.
pub async fn serve<E>(server: App<E>, bind: &str, shutdown: impl std::future::Future<Output = ()> + Send + 'static) -> std::io::Result<()>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(addr = %listener.local_addr()?, "listening");
    axum::serve(listener, router(server).into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(shutdown)
        .await
}
