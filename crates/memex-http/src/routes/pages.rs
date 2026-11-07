//! Routes for wiki pages. Writes are proposals, so they answer as `/proposals` does.
use super::entities::{invalid, result_of};
use super::{App, RequestId, authed, body};
use crate::error::ApiError;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use memex_api::{ErrorBody, ListParams, ProposalQuery, ProposalResult, page_of};
use memex_core::{NewPage, Server, ServerEdges};
use memex_types::{ObjectId, VaultId};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        list_pages,
        create_page,
        import_page,
        redirect,
        unresolved,
        get_page,
        export_page,
        edit_page,
        rename_page,
        delete_page,
        revert_page,
        revisions,
        diff,
        backlinks,
        links,
        neighborhood
    ),
    components(schemas(
        memex_api::PageSummaryDto,
        memex_api::PageSummaryPage,
        memex_api::NewPageBody,
        memex_api::PageEditBody,
        memex_api::PageRenameBody,
        memex_api::PageRevertBody,
        memex_api::PageDeleteBody,
        memex_api::PageImportBody,
        memex_api::PageProposalResult,
        memex_api::PageImportResult,
        memex_api::BlockDto,
        memex_api::PageClaimDto,
        memex_api::PageDetailDto,
        memex_api::RevisionDto,
        memex_api::RevisionPage,
        memex_api::BlockChangeDto,
        memex_api::PageDiffDto,
        memex_api::BacklinkDto,
        memex_api::BacklinkPage,
        memex_api::OutgoingLinkDto,
        memex_api::OutgoingLinkPage,
        memex_api::UnresolvedLinkDto,
        memex_api::UnresolvedLinkPage,
        memex_api::PageRedirectDto,
        memex_api::NeighborDto,
        memex_api::NeighborPage
    ))
)]
pub(super) struct PageDoc;

pub(super) fn add_routes<E>(api: Router<App<E>>) -> Router<App<E>>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    api.route("/vaults/{id}/pages", get(list_pages::<E>).post(create_page::<E>))
        .route("/vaults/{id}/pages/import", post(import_page::<E>))
        .route("/vaults/{id}/pages/unresolved-links", get(unresolved::<E>))
        .route("/vaults/{id}/pages/by-slug/{slug}", get(redirect::<E>))
        .route("/vaults/{id}/pages/{pid}", get(get_page::<E>))
        .route("/vaults/{id}/pages/{pid}/export", get(export_page::<E>))
        .route("/vaults/{id}/pages/{pid}/edit", post(edit_page::<E>))
        .route("/vaults/{id}/pages/{pid}/rename", post(rename_page::<E>))
        .route("/vaults/{id}/pages/{pid}/delete", post(delete_page::<E>))
        .route("/vaults/{id}/pages/{pid}/revert", post(revert_page::<E>))
        .route("/vaults/{id}/pages/{pid}/revisions", get(revisions::<E>))
        .route("/vaults/{id}/pages/{pid}/diff", get(diff::<E>))
        .route("/vaults/{id}/pages/{pid}/backlinks", get(backlinks::<E>))
        .route("/vaults/{id}/pages/{pid}/links", get(links::<E>))
        .route("/vaults/{id}/objects/{oid}/neighborhood", get(neighborhood::<E>))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/pages", tag = "pages", params(("id" = String, Path), ListParams), responses((status = 200, body = memex_api::PageSummaryPage), (status = 404, body = ErrorBody)))]
async fn list_pages<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::PageSummaryPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = params.validated().map_err(|m| invalid(m, &rid))?;
    let limit = l.limit;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.list_wiki_pages(c, &VaultId(id), l.cursor.map(ObjectId).as_ref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, limit, |p| p.header.id.0.clone());
    Ok(Json(memex_api::PageSummaryPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

/// A person's page commits at once. An agent's waits for review until it has earned auto-commit. The response says which.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/pages", tag = "pages", params(("id" = String, Path), ProposalQuery), request_body = memex_api::NewPageBody, responses((status = 200, body = memex_api::PageProposalResult), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn create_page<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ProposalQuery>,
    parsed: Result<Json<memex_api::NewPageBody>, JsonRejection>,
) -> Result<Json<memex_api::PageProposalResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let new = NewPage {
        title: b.title,
        slug: b.slug,
        body: b.body,
        rationale: b.rationale,
        idempotency_key: b.idempotency_key,
    };
    let dry = q.dry_run.unwrap_or(false);
    let (page_id, out) = authed(server, headers, &rid, move |s: &Server<E>, c| s.propose_new_page(c, &VaultId(id), new, dry)).await?;
    Ok(Json(memex_api::PageProposalResult {
        page_id: page_id.0,
        result: result_of(out),
    }))
}

/// Reads a page's Markdown file. Links reach their pages by slug, so files can arrive in any order.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/pages/import", tag = "pages", params(("id" = String, Path)), request_body = memex_api::PageImportBody, responses((status = 200, body = memex_api::PageImportResult), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn import_page<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    parsed: Result<Json<memex_api::PageImportBody>, JsonRejection>,
) -> Result<Json<memex_api::PageImportResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let out = authed(server, headers, &rid, move |s: &Server<E>, c| {
        s.import_page(c, &VaultId(id), &b.markdown, b.base_version, b.rationale, b.idempotency_key)
    })
    .await?;
    Ok(Json(memex_api::PageImportResult {
        page_id: out.page_id.0,
        created: out.created,
        result: result_of(out.outcome),
    }))
}

/// Where a slug leads, whether current or old.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/pages/by-slug/{slug}", tag = "pages", params(("id" = String, Path), ("slug" = String, Path)), responses((status = 200, body = memex_api::PageRedirectDto), (status = 404, body = ErrorBody)))]
async fn redirect<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, slug)): Path<(String, String)>,
) -> Result<Json<memex_api::PageRedirectDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let r = authed(server, headers, &rid, move |s, c| s.page_redirect(c, &VaultId(id), &slug)).await?;
    Ok(Json(memex_api::PageRedirectDto {
        slug: r.slug,
        page_id: r.page_id.0,
        current_slug: r.current_slug,
        redirected: r.redirected,
    }))
}

/// Wiki links whose slug no page holds, and claim references whose claim doesn't exist.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/pages/unresolved-links", tag = "pages", params(("id" = String, Path), ListParams), responses((status = 200, body = memex_api::UnresolvedLinkPage), (status = 404, body = ErrorBody)))]
async fn unresolved<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::UnresolvedLinkPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = params.validated().map_err(|m| invalid(m, &rid))?;
    let limit = l.limit;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.unresolved_page_links(c, &VaultId(id), l.cursor.map(ObjectId).as_ref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, limit, |u| u.edge_id.0.clone());
    Ok(Json(memex_api::UnresolvedLinkPage {
        items: items
            .into_iter()
            .map(|u| memex_api::UnresolvedLinkDto {
                edge_id: u.edge_id.0,
                kind: u.kind,
                from_page: u.from_page.0,
                block_id: u.block_id.map(|b| b.0),
                target: u.target,
            })
            .collect(),
        next_cursor,
    }))
}

/// The page now, or at a past revision. Claim references show what stands today either way.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/pages/{pid}", tag = "pages", params(("id" = String, Path), ("pid" = String, Path), memex_api::PageReadParams), responses((status = 200, body = memex_api::PageDetailDto), (status = 404, body = ErrorBody)))]
async fn get_page<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, pid)): Path<(String, String)>,
    Query(q): Query<memex_api::PageReadParams>,
) -> Result<Json<memex_api::PageDetailDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let v = authed(server, headers, &rid, move |s, c| s.get_wiki_page(c, &VaultId(id), &ObjectId(pid), q.revision)).await?;
    Ok(Json(memex_api::page_detail(v)))
}

/// The page as a Markdown file with front matter, which import reads back.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/pages/{pid}/export", tag = "pages", params(("id" = String, Path), ("pid" = String, Path)), responses((status = 200, body = String, content_type = "text/markdown"), (status = 404, body = ErrorBody)))]
async fn export_page<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, pid)): Path<(String, String)>,
) -> Result<Response, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let text = authed(server, headers, &rid, move |s, c| s.export_page(c, &VaultId(id), &ObjectId(pid))).await?;
    Ok(([(header::CONTENT_TYPE, HeaderValue::from_static("text/markdown; charset=utf-8"))], text).into_response())
}

/// A stale `base_version` answers with a conflict that carries the page's current state.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/pages/{pid}/edit", tag = "pages", params(("id" = String, Path), ("pid" = String, Path), ProposalQuery), request_body = memex_api::PageEditBody, responses((status = 200, body = ProposalResult), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn edit_page<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, pid)): Path<(String, String)>,
    Query(q): Query<ProposalQuery>,
    parsed: Result<Json<memex_api::PageEditBody>, JsonRejection>,
) -> Result<Json<ProposalResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let dry = q.dry_run.unwrap_or(false);
    let out = authed(server, headers, &rid, move |s: &Server<E>, c| {
        s.propose_page_edit(c, &VaultId(id), &ObjectId(pid), b.base_version, b.body, b.rationale, b.idempotency_key, dry)
    })
    .await?;
    Ok(Json(result_of(out)))
}

/// A new title and slug. The old slug keeps leading here, so no link breaks.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/pages/{pid}/rename", tag = "pages", params(("id" = String, Path), ("pid" = String, Path)), request_body = memex_api::PageRenameBody, responses((status = 200, body = ProposalResult), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn rename_page<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, pid)): Path<(String, String)>,
    parsed: Result<Json<memex_api::PageRenameBody>, JsonRejection>,
) -> Result<Json<ProposalResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let out = authed(server, headers, &rid, move |s: &Server<E>, c| {
        s.propose_page_rename(c, &VaultId(id), &ObjectId(pid), b.base_version, b.title, b.slug, b.rationale, b.idempotency_key)
    })
    .await?;
    Ok(Json(result_of(out)))
}

/// Proposes deleting the page. It always waits for review. Links to it read as broken until a page takes the slug.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/pages/{pid}/delete", tag = "pages", params(("id" = String, Path), ("pid" = String, Path)), request_body = memex_api::PageDeleteBody, responses((status = 200, body = ProposalResult), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn delete_page<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, pid)): Path<(String, String)>,
    parsed: Result<Json<memex_api::PageDeleteBody>, JsonRejection>,
) -> Result<Json<ProposalResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let out = authed(server, headers, &rid, move |s: &Server<E>, c| {
        s.propose_page_delete(c, &VaultId(id), &ObjectId(pid), b.base_version, b.rationale, b.idempotency_key)
    })
    .await?;
    Ok(Json(result_of(out)))
}

/// Makes an old revision current again, as a new revision.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/pages/{pid}/revert", tag = "pages", params(("id" = String, Path), ("pid" = String, Path)), request_body = memex_api::PageRevertBody, responses((status = 200, body = ProposalResult), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn revert_page<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, pid)): Path<(String, String)>,
    parsed: Result<Json<memex_api::PageRevertBody>, JsonRejection>,
) -> Result<Json<ProposalResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let out = authed(server, headers, &rid, move |s: &Server<E>, c| {
        s.propose_page_revert(c, &VaultId(id), &ObjectId(pid), b.base_version, b.revision, b.rationale, b.idempotency_key)
    })
    .await?;
    Ok(Json(result_of(out)))
}

/// Oldest first. A deleted page's history is still here.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/pages/{pid}/revisions", tag = "pages", params(("id" = String, Path), ("pid" = String, Path), ListParams), responses((status = 200, body = memex_api::RevisionPage), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn revisions<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, pid)): Path<(String, String)>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::RevisionPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = params.validated().map_err(|m| invalid(m, &rid))?;
    let limit = l.limit;
    let after = match l.cursor {
        Some(c) => Some(c.parse::<u32>().map_err(|_| invalid("that cursor isn't one this API gave out".into(), &rid))?),
        None => None,
    };
    let rows = authed(server, headers, &rid, move |s, c| {
        s.list_page_revisions(c, &VaultId(id), &ObjectId(pid), after, limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, limit, |r| r.revision.to_string());
    Ok(Json(memex_api::RevisionPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

/// What changed between two revisions, block by block.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/pages/{pid}/diff", tag = "pages", params(("id" = String, Path), ("pid" = String, Path), memex_api::PageDiffParams), responses((status = 200, body = memex_api::PageDiffDto), (status = 404, body = ErrorBody)))]
async fn diff<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, pid)): Path<(String, String)>,
    Query(q): Query<memex_api::PageDiffParams>,
) -> Result<Json<memex_api::PageDiffDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let d = authed(server, headers, &rid, move |s, c| s.page_diff(c, &VaultId(id), &ObjectId(pid), q.from, q.to)).await?;
    Ok(Json(memex_api::PageDiffDto {
        from: d.from,
        to: d.to,
        title: d.title.map(|(a, b)| vec![a, b]),
        slug: d.slug.map(|(a, b)| vec![a, b]),
        blocks: d
            .blocks
            .into_iter()
            .map(|c| memex_api::BlockChangeDto {
                block_id: c.block_id.0,
                change: c.change,
                before: c.before,
                after: c.after,
            })
            .collect(),
    }))
}

/// What links here, through any slug the page has ever had.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/pages/{pid}/backlinks", tag = "pages", params(("id" = String, Path), ("pid" = String, Path), ListParams), responses((status = 200, body = memex_api::BacklinkPage), (status = 404, body = ErrorBody)))]
async fn backlinks<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, pid)): Path<(String, String)>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::BacklinkPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = params.validated().map_err(|m| invalid(m, &rid))?;
    let limit = l.limit;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.page_backlinks(c, &VaultId(id), &ObjectId(pid), l.cursor.map(ObjectId).as_ref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, limit, |b| b.edge_id.0.clone());
    Ok(Json(memex_api::BacklinkPage {
        items: items
            .into_iter()
            .map(|b| memex_api::BacklinkDto {
                edge_id: b.edge_id.0,
                from_page: b.from_page.0,
                from_title: b.from_title,
                from_slug: b.from_slug,
                block_id: b.block_id.map(|x| x.0),
                via_slug: b.via_slug,
            })
            .collect(),
        next_cursor,
    }))
}

/// What this page links to and cites, each saying whether it leads anywhere.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/pages/{pid}/links", tag = "pages", params(("id" = String, Path), ("pid" = String, Path), ListParams), responses((status = 200, body = memex_api::OutgoingLinkPage), (status = 404, body = ErrorBody)))]
async fn links<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, pid)): Path<(String, String)>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::OutgoingLinkPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = params.validated().map_err(|m| invalid(m, &rid))?;
    let limit = l.limit;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.page_links(c, &VaultId(id), &ObjectId(pid), l.cursor.map(ObjectId).as_ref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, limit, |o| o.edge_id.0.clone());
    Ok(Json(memex_api::OutgoingLinkPage {
        items: items
            .into_iter()
            .map(|o| memex_api::OutgoingLinkDto {
                edge_id: o.edge_id.0,
                kind: o.kind,
                block_id: o.block_id.map(|b| b.0),
                target: o.target,
                to_page: o.to_page.map(|p| p.0),
                resolved: o.resolved,
            })
            .collect(),
        next_cursor,
    }))
}

/// Every link at one object, both ways, one hop. Kinds default to wiki links, claim references and contradictions.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/objects/{oid}/neighborhood", tag = "pages", params(("id" = String, Path), ("oid" = String, Path), ListParams, memex_api::NeighborhoodParams), responses((status = 200, body = memex_api::NeighborPage), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn neighborhood<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, oid)): Path<(String, String)>,
    Query(params): Query<ListParams>,
    Query(extra): Query<memex_api::NeighborhoodParams>,
) -> Result<Json<memex_api::NeighborPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = params.validated().map_err(|m| invalid(m, &rid))?;
    let (limit, kinds) = (l.limit, extra.kinds());
    let rows = authed(server, headers, &rid, move |s, c| {
        let kinds: Vec<&str> = kinds.iter().map(String::as_str).collect();
        s.neighborhood(c, &VaultId(id), &ObjectId(oid), &kinds, l.cursor.map(ObjectId).as_ref(), l.limit + 1)
    })
    .await?;
    Ok(Json(memex_api::neighbor_page(rows, limit)))
}
