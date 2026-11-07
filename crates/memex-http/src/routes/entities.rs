//! Routes for the entity registry. Writes are proposals, so they answer as `/proposals` does.
use super::{App, RequestId, authed, body};
use crate::error::ApiError;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use memex_api::{ErrorBody, ListParams, ProposalQuery, ProposalResult, page_of};
use memex_core::{EntityEdit, FieldInput, NewEntity, ProposalOutcome, Server, ServerEdges, SplitRequest};
use memex_types::{ObjectId, VaultId};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        list_types,
        submit_type,
        type_versions,
        preview_type,
        list_entities,
        create_entity,
        resolve,
        same_as,
        get_entity,
        edit_entity,
        redirect,
        merge,
        split
    ),
    components(schemas(
        memex_api::EntityTypeDto,
        memex_api::EntityTypePage,
        memex_api::EntityTypeVersions,
        memex_api::NewEntityTypeBody,
        memex_api::TypePreviewBody,
        memex_api::TypePreviewDto,
        memex_api::EvidenceInputDto,
        memex_api::ReplacesDto,
        memex_api::FieldInputDto,
        memex_api::NewEntityBody,
        memex_api::EntityEditBody,
        memex_api::MergeBody,
        memex_api::SplitBody,
        memex_api::EntityProposalResult,
        memex_api::EntityDto,
        memex_api::EntityPage,
        memex_api::FieldEvidenceDto,
        memex_api::FieldClaimDto,
        memex_api::EntityFieldDto,
        memex_api::EntityDetailDto,
        memex_api::RedirectDto,
        memex_api::CandidateDto,
        memex_api::ResolveResult,
        memex_api::SameAsDto,
        memex_api::SameAsPage
    ))
)]
pub(super) struct EntityDoc;

pub(super) fn add_routes<E>(api: Router<App<E>>) -> Router<App<E>>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    api.route("/vaults/{id}/entity-types", get(list_types::<E>).post(submit_type::<E>))
        .route("/vaults/{id}/entity-types/{name}/versions", get(type_versions::<E>))
        .route("/vaults/{id}/entity-types/{name}/preview", post(preview_type::<E>))
        .route("/vaults/{id}/entities", get(list_entities::<E>).post(create_entity::<E>))
        .route("/vaults/{id}/entities/resolve", get(resolve::<E>))
        .route("/vaults/{id}/entities/same-as", get(same_as::<E>))
        .route("/vaults/{id}/entities/{eid}", get(get_entity::<E>))
        .route("/vaults/{id}/entities/{eid}/edit", post(edit_entity::<E>))
        .route("/vaults/{id}/entities/{eid}/redirect", get(redirect::<E>))
        .route("/vaults/{id}/entities/{eid}/merge", post(merge::<E>))
        .route("/vaults/{id}/entities/{eid}/split", post(split::<E>))
}

pub(super) fn invalid(m: String, rid: &RequestId) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid", m, &rid.0)
}

pub(super) fn text_of<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

pub(super) fn result_of(out: ProposalOutcome) -> ProposalResult {
    memex_api::proposal_result(out)
}

fn field_inputs(fields: Vec<memex_api::FieldInputDto>, rid: &RequestId) -> Result<Vec<FieldInput>, ApiError> {
    fields
        .into_iter()
        .map(|f| {
            let p = f.parts().map_err(|m| invalid(m, rid))?;
            Ok(FieldInput {
                field: p.field,
                value: p.value,
                evidence: p.evidence,
                replaces: p.replaces,
            })
        })
        .collect()
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/entity-types", tag = "entities", params(("id" = String, Path), ListParams), responses((status = 200, body = memex_api::EntityTypePage), (status = 404, body = ErrorBody)))]
async fn list_types<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::EntityTypePage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = params.validated().map_err(|m| invalid(m, &rid))?;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.list_entity_types(c, &VaultId(id), l.cursor.as_deref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, l.limit, |t| t.name.clone());
    Ok(Json(memex_api::EntityTypePage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

/// A new type or the next version of one. An additive change by a person commits, and a breaking one waits for review. The response says which.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/entity-types", tag = "entities", params(("id" = String, Path)), request_body = memex_api::NewEntityTypeBody, responses((status = 200, body = ProposalResult), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn submit_type<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    parsed: Result<Json<memex_api::NewEntityTypeBody>, JsonRejection>,
) -> Result<Json<ProposalResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let out = authed(server, headers, &rid, move |s, c| {
        s.propose_entity_type(c, &VaultId(id), &b.name, b.base_version, b.schema, b.rationale, b.idempotency_key)
    })
    .await?;
    Ok(Json(result_of(out)))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/entity-types/{name}/versions", tag = "entities", params(("id" = String, Path), ("name" = String, Path)), responses((status = 200, body = memex_api::EntityTypeVersions), (status = 404, body = ErrorBody)))]
async fn type_versions<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, name)): Path<(String, String)>,
) -> Result<Json<memex_api::EntityTypeVersions>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let rows = authed(server, headers, &rid, move |s, c| s.entity_type_history(c, &VaultId(id), &name)).await?;
    Ok(Json(memex_api::EntityTypeVersions {
        items: rows.into_iter().map(Into::into).collect(),
    }))
}

/// Classifies a change as additive or breaking and says what would happen to it, writing nothing. A submission of the same schema gets the same answer.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/entity-types/{name}/preview", tag = "entities", params(("id" = String, Path), ("name" = String, Path)), request_body = memex_api::TypePreviewBody, responses((status = 200, body = memex_api::TypePreviewDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn preview_type<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, name)): Path<(String, String)>,
    parsed: Result<Json<memex_api::TypePreviewBody>, JsonRejection>,
) -> Result<Json<memex_api::TypePreviewDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let d = authed(server, headers, &rid, move |s, c| s.preview_entity_type(c, &VaultId(id), &name, &b.schema)).await?;
    Ok(Json(memex_api::TypePreviewDto {
        problem: d.preview.problem,
        change: d.preview.change.as_str().into(),
        reasons: d.preview.reasons,
        current_version: d.preview.current_version,
        next_version: d.preview.next_version,
        decision: text_of(&d.risk),
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/entities", tag = "entities", params(("id" = String, Path), memex_api::EntityListParams), responses((status = 200, body = memex_api::EntityPage), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn list_entities<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<memex_api::EntityPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let r = memex_api::entity_list_request(pairs).map_err(|m| invalid(m, &rid))?;
    let limit = r.list.limit;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.list_entities(
            c,
            &VaultId(id),
            &r.type_name,
            &r.filters,
            r.list.cursor.map(ObjectId).as_ref(),
            r.list.limit + 1,
        )
    })
    .await?;
    let (items, next_cursor) = page_of(rows, limit, |e| e.header.id.0.clone());
    Ok(Json(memex_api::EntityPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(post, path = "/api/v1/vaults/{id}/entities", tag = "entities", params(("id" = String, Path), ProposalQuery), request_body = memex_api::NewEntityBody, responses((status = 200, body = memex_api::EntityProposalResult), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn create_entity<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ProposalQuery>,
    parsed: Result<Json<memex_api::NewEntityBody>, JsonRejection>,
) -> Result<Json<memex_api::EntityProposalResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let fields = field_inputs(b.fields, &rid)?;
    let new = NewEntity {
        type_name: b.type_name,
        aliases: b.aliases,
        fields,
        rationale: b.rationale,
        idempotency_key: b.idempotency_key,
    };
    let dry = q.dry_run.unwrap_or(false);
    let (entity_id, out) = authed(server, headers, &rid, move |s, c| s.propose_new_entity(c, &VaultId(id), new, dry)).await?;
    Ok(Json(memex_api::EntityProposalResult {
        entity_id: entity_id.0,
        result: result_of(out),
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/entities/resolve", tag = "entities", params(("id" = String, Path), memex_api::ResolveParams), responses((status = 200, body = memex_api::ResolveResult), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn resolve<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(p): Query<memex_api::ResolveParams>,
) -> Result<Json<memex_api::ResolveResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let limit = p.limit().map_err(|m| invalid(m, &rid))?;
    let r = authed(server, headers, &rid, move |s, c| {
        s.resolve_mention(c, &VaultId(id), &p.mention, p.type_name.as_deref(), limit)
    })
    .await?;
    Ok(Json(memex_api::resolve_result(r)))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/entities/same-as", tag = "entities", params(("id" = String, Path), memex_api::SameAsParams), responses((status = 200, body = memex_api::SameAsPage), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn same_as<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(p): Query<memex_api::SameAsParams>,
) -> Result<Json<memex_api::SameAsPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let list = ListParams {
        cursor: p.cursor,
        limit: p.limit,
        sort: None,
    }
    .validated()
    .map_err(|m| invalid(m, &rid))?;
    let limit = list.limit;
    let type_name = p.type_name;
    let rows = authed(server, headers, &rid, move |s, c| {
        let after = list.cursor.as_deref().and_then(|cursor| cursor.split_once('\n'));
        s.same_as_suggestions(c, &VaultId(id), &type_name, after, list.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, limit, |s| format!("{}\n{}", s.field, s.text));
    Ok(Json(memex_api::SameAsPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

/// An old id gets the entity it now leads to, with `redirected_from` set.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/entities/{eid}", tag = "entities", params(("id" = String, Path), ("eid" = String, Path)), responses((status = 200, body = memex_api::EntityDetailDto), (status = 404, body = ErrorBody)))]
async fn get_entity<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, eid)): Path<(String, String)>,
) -> Result<Json<memex_api::EntityDetailDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let view = authed(server, headers, &rid, move |s, c| s.get_entity(c, &VaultId(id), &ObjectId(eid))).await?;
    Ok(Json(memex_api::entity_detail(view)))
}

#[utoipa::path(post, path = "/api/v1/vaults/{id}/entities/{eid}/edit", tag = "entities", params(("id" = String, Path), ("eid" = String, Path), ProposalQuery), request_body = memex_api::EntityEditBody, responses((status = 200, body = ProposalResult), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn edit_entity<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, eid)): Path<(String, String)>,
    Query(q): Query<ProposalQuery>,
    parsed: Result<Json<memex_api::EntityEditBody>, JsonRejection>,
) -> Result<Json<ProposalResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let fields = field_inputs(b.fields, &rid)?;
    let edit = EntityEdit {
        base_version: b.base_version,
        aliases: b.aliases,
        fields,
        rationale: b.rationale,
        idempotency_key: b.idempotency_key,
    };
    let dry = q.dry_run.unwrap_or(false);
    let out = authed(server, headers, &rid, move |s, c| {
        s.propose_entity_edit(c, &VaultId(id), &ObjectId(eid), edit, dry)
    })
    .await?;
    Ok(Json(result_of(out)))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/entities/{eid}/redirect", tag = "entities", params(("id" = String, Path), ("eid" = String, Path)), responses((status = 200, body = memex_api::RedirectDto), (status = 404, body = ErrorBody)))]
async fn redirect<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, eid)): Path<(String, String)>,
) -> Result<Json<memex_api::RedirectDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let r = authed(server, headers, &rid, move |s, c| s.entity_redirect(c, &VaultId(id), &ObjectId(eid))).await?;
    Ok(Json(memex_api::RedirectDto {
        id: r.id.0,
        current_id: r.current_id.0,
        chain: r.chain.into_iter().map(|c| c.0).collect(),
    }))
}

/// Proposes merging this entity into another. It always waits for a person's approval.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/entities/{eid}/merge", tag = "entities", params(("id" = String, Path), ("eid" = String, Path)), request_body = memex_api::MergeBody, responses((status = 200, body = ProposalResult), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn merge<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, eid)): Path<(String, String)>,
    parsed: Result<Json<memex_api::MergeBody>, JsonRejection>,
) -> Result<Json<ProposalResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let out = authed(server, headers, &rid, move |s: &Server<E>, c| {
        s.propose_merge(c, &VaultId(id), &ObjectId(eid), &ObjectId(b.into), b.rationale, b.idempotency_key)
    })
    .await?;
    Ok(Json(result_of(out)))
}

/// Proposes moving claims and aliases to a new entity. It always waits for a person's approval.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/entities/{eid}/split", tag = "entities", params(("id" = String, Path), ("eid" = String, Path)), request_body = memex_api::SplitBody, responses((status = 200, body = memex_api::EntityProposalResult), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn split<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, eid)): Path<(String, String)>,
    parsed: Result<Json<memex_api::SplitBody>, JsonRejection>,
) -> Result<Json<memex_api::EntityProposalResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let fields = field_inputs(b.fields, &rid)?;
    let request = SplitRequest {
        base_version: b.base_version,
        moved: b.moved.into_iter().map(ObjectId).collect(),
        aliases: b.aliases,
        fields,
        rationale: b.rationale,
        idempotency_key: b.idempotency_key,
    };
    let (entity_id, out) = authed(server, headers, &rid, move |s: &Server<E>, c| {
        s.propose_split(c, &VaultId(id), &ObjectId(eid), request)
    })
    .await?;
    Ok(Json(memex_api::EntityProposalResult {
        entity_id: entity_id.0,
        result: result_of(out),
    }))
}
