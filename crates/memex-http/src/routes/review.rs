//! Routes for the review gate: the queue and its counts, one item with its reviewer verdict, decisions, and the auto-commit table.
use super::{App, RequestId, authed, body, list};
use crate::error::ApiError;
use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use memex_api::{ErrorBody, ListParams, page_of};
use memex_core::{AutoCommitEntry, BatchDecision, ReviewDetail, ReviewEvidence, ReviewItem, Server, ServerEdges};
use memex_types::{ChangeSetState, ObjectId, Op, ReviewFilter, VaultId, Verdict, VerdictFilter};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(review_queue, review_counts, review_item, edit_approve, decide_batch, auto_commit_table, disable_auto_commit),
    components(schemas(
        memex_api::ReviewVerdictDto,
        memex_api::ReviewItemDto,
        memex_api::ReviewItemPage,
        memex_api::ReviewEvidenceDto,
        memex_api::ReviewDetailDto,
        memex_api::QueueCountDto,
        memex_api::QueueCountsDto,
        memex_api::EditApproveBody,
        memex_api::DisableBody,
        memex_api::BatchBody,
        memex_api::BatchDecisionBody,
        memex_api::BatchResultDto,
        memex_api::BatchResultsDto,
        memex_api::AutoCommitDto,
        memex_api::AutoCommitChangeDto,
        memex_api::AutoCommitPage
    ))
)]
pub(super) struct ReviewDoc;

pub(super) fn add_routes<E>(api: Router<App<E>>) -> Router<App<E>>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    api.route("/vaults/{id}/review/queue", get(review_queue::<E>))
        .route("/vaults/{id}/review/counts", get(review_counts::<E>))
        .route("/vaults/{id}/review/items/{cs}", get(review_item::<E>))
        .route("/vaults/{id}/review/items/{cs}/edit-approve", post(edit_approve::<E>))
        .route("/vaults/{id}/review/batch", post(decide_batch::<E>))
        .route("/vaults/{id}/review/auto-commit", get(auto_commit_table::<E>))
        .route("/vaults/{id}/review/auto-commit/{combination}/disable", post(disable_auto_commit::<E>))
}

fn invalid(message: impl Into<String>, rid: &RequestId) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid", message, &rid.0)
}

fn filter_of(q: memex_api::ReviewQueueQuery, rid: &RequestId) -> Result<ReviewFilter, ApiError> {
    let state = match q.state.as_deref() {
        None => None,
        Some("pending") => Some(ChangeSetState::Pending),
        Some("committed") => Some(ChangeSetState::Committed),
        Some("rejected") => Some(ChangeSetState::Rejected),
        Some(other) => return Err(invalid(format!("unknown state {other}"), rid)),
    };
    let verdict = match q.verdict.as_deref() {
        None => None,
        Some("approve") => Some(VerdictFilter::Is(Verdict::Approve)),
        Some("reject") => Some(VerdictFilter::Is(Verdict::Reject)),
        Some("abstain") => Some(VerdictFilter::Is(Verdict::Abstain)),
        Some("none") => Some(VerdictFilter::None),
        Some(other) => return Err(invalid(format!("unknown verdict {other}"), rid)),
    };
    Ok(ReviewFilter {
        state,
        extractor: q.extractor,
        prompt_version: q.prompt_version,
        item_type: q.item_type,
        source_id: q.source_id.map(ObjectId),
        proposer: q.proposer,
        verdict,
    })
}

fn item_dto(i: ReviewItem) -> memex_api::ReviewItemDto {
    memex_api::ReviewItemDto {
        changeset: i.changeset.into(),
        verdict: i.verdict.map(Into::into),
    }
}

fn entry_dto(e: AutoCommitEntry) -> memex_api::AutoCommitDto {
    memex_api::AutoCommitDto {
        id: e.row.header.id.0,
        extractor: e.row.extractor,
        prompt_version: e.row.prompt_version,
        item_type: e.row.item_type,
        correct: e.row.correct,
        incorrect: e.row.incorrect,
        total_correct: e.row.total_correct,
        total_incorrect: e.row.total_incorrect,
        precision: e.precision,
        wilson_bound: e.wilson_bound,
        on: e.on,
        history: e.row.history.into_iter().map(Into::into).collect(),
    }
}

fn evidence_dto(e: ReviewEvidence) -> memex_api::ReviewEvidenceDto {
    memex_api::ReviewEvidenceDto {
        claim_id: e.claim_id.0,
        segment_id: e.segment_id.0,
        quote: e.quote,
        source_id: e.source_id.map(|i| i.0),
        source_name: e.source_name,
        trust_tier: e.trust_tier,
        rendition_id: e.rendition_id.map(|i| i.0),
        page_from: e.page_from,
        page_to: e.page_to,
        superseded: e.superseded,
        page_flags: e.page_flags.iter().filter_map(|f| serde_json::to_value(f).ok()).collect(),
    }
}

fn detail_dto(d: ReviewDetail) -> memex_api::ReviewDetailDto {
    memex_api::ReviewDetailDto {
        changeset: d.item.changeset.into(),
        verdict: d.item.verdict.map(Into::into),
        evidence: d.evidence.into_iter().map(evidence_dto).collect(),
        combinations: d.combinations.into_iter().map(entry_dto).collect(),
        needs_a_person: d.needs_a_person,
        auto_commit_block: d.auto_commit_block,
    }
}

fn ops_of(values: Vec<serde_json::Value>, rid: &RequestId) -> Result<Vec<Op>, ApiError> {
    values
        .into_iter()
        .map(|v| serde_json::from_value(v).map_err(|e| invalid(format!("a bad op: {e}"), rid)))
        .collect()
}

/// The queue with `state=pending`, or any other ChangeSets the filters ask for, in creation order. Each comes with the reviewer agent's verdict if it has one.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/review/queue", tag = "review", params(("id" = String, Path), ListParams, memex_api::ReviewQueueQuery), responses((status = 200, body = memex_api::ReviewItemPage), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn review_queue<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
    Query(query): Query<memex_api::ReviewQueueQuery>,
) -> Result<Json<memex_api::ReviewItemPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let filter = filter_of(query, &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.review_queue(c, &VaultId(id), &filter, l.cursor.map(ObjectId).as_ref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, l.limit, |x| x.changeset.header.id.0.clone());
    Ok(Json(memex_api::ReviewItemPage {
        items: items.into_iter().map(item_dto).collect(),
        next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/review/counts", tag = "review", params(("id" = String, Path)), responses((status = 200, body = memex_api::QueueCountsDto), (status = 404, body = ErrorBody)))]
async fn review_counts<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<memex_api::QueueCountsDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let vault = VaultId(id);
    let (counts, pending) = authed(server, headers, &rid, move |s, c| {
        let counts = s.review_counts(c, &vault)?;
        let pending = s.pending_count(c, &vault)?;
        Ok((counts, pending))
    })
    .await?;
    Ok(Json(memex_api::QueueCountsDto {
        pending,
        groups: counts.into_iter().map(Into::into).collect(),
    }))
}

/// One item: the proposal, the reviewer agent's verdict, what its quotes rest on, and where its combination stands.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/review/items/{cs}", tag = "review", params(("id" = String, Path), ("cs" = String, Path)), responses((status = 200, body = memex_api::ReviewDetailDto), (status = 404, body = ErrorBody)))]
async fn review_item<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, cs)): Path<(String, String)>,
) -> Result<Json<memex_api::ReviewDetailDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let detail = authed(server, headers, &rid, move |s, c| s.review_item(c, &VaultId(id), &ObjectId(cs))).await?;
    Ok(Json(detail_dto(detail)))
}

/// Approves a person's corrected version. What was proposed stays on record, and it counts against the extractor. Only a person can do this.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/review/items/{cs}/edit-approve", tag = "review", params(("id" = String, Path), ("cs" = String, Path)), request_body = memex_api::EditApproveBody, responses((status = 200, body = memex_api::ChangeSetDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn edit_approve<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, cs)): Path<(String, String)>,
    parsed: Result<Json<memex_api::EditApproveBody>, JsonRejection>,
) -> Result<Json<memex_api::ChangeSetDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let ops = ops_of(b.ops, &rid)?;
    let row = authed(server, headers, &rid, move |s, c| {
        s.approve_edited(c, &VaultId(id), &ObjectId(cs), ops, b.reason)
    })
    .await?;
    Ok(Json(row.into()))
}

/// Up to 200 decisions, each attributed and each on its own, so one conflict leaves the others as they are. A malformed request refuses the lot.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/review/batch", tag = "review", params(("id" = String, Path)), request_body = memex_api::BatchBody, responses((status = 200, body = memex_api::BatchResultsDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn decide_batch<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    parsed: Result<Json<memex_api::BatchBody>, JsonRejection>,
) -> Result<Json<memex_api::BatchResultsDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let mut decisions = vec![];
    for d in b.decisions {
        let cs = ObjectId(d.id);
        decisions.push(match d.decision.as_str() {
            "approve" => BatchDecision::Approve { id: cs, reason: d.reason },
            "reject" => BatchDecision::Reject {
                id: cs,
                reason: d.reason.unwrap_or_default(),
            },
            "edit_approve" => BatchDecision::ApproveEdited {
                id: cs,
                ops: ops_of(d.ops.ok_or_else(|| invalid("edit_approve needs ops", &rid))?, &rid)?,
                reason: d.reason,
            },
            other => return Err(invalid(format!("unknown decision {other}"), &rid)),
        });
    }
    let ids: Vec<String> = decisions.iter().map(|d| d.id().0.clone()).collect();
    let outcomes = authed(server, headers, &rid, move |s, c| s.decide_batch(c, &VaultId(id), decisions)).await?;
    let results = ids
        .into_iter()
        .zip(outcomes)
        .map(|(id, outcome)| match outcome {
            Ok(row) => memex_api::BatchResultDto {
                id,
                changeset: Some(row.into()),
                error_code: None,
                error_message: None,
            },
            Err(e) => {
                let (code, message) = ApiError::from_server(e, &rid.0).code_and_message();
                memex_api::BatchResultDto {
                    id,
                    changeset: None,
                    error_code: Some(code.to_string()),
                    error_message: Some(message),
                }
            }
        })
        .collect();
    Ok(Json(memex_api::BatchResultsDto { results }))
}

/// Each combination of extractor, prompt version and type: its precision, Wilson bound, whether it's on, and every time that changed, with the threshold it's judged by.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/review/auto-commit", tag = "review", params(("id" = String, Path), ListParams), responses((status = 200, body = memex_api::AutoCommitPage), (status = 404, body = ErrorBody)))]
async fn auto_commit_table<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::AutoCommitPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let table = authed(server, headers, &rid, move |s, c| {
        s.auto_commit_table(c, &VaultId(id), l.cursor.map(ObjectId).as_ref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(table.entries, l.limit, |e| e.row.header.id.0.clone());
    Ok(Json(memex_api::AutoCommitPage {
        threshold: table.threshold,
        items: items.into_iter().map(entry_dto).collect(),
        next_cursor,
    }))
}

/// Stops auto-commit for this combination at once. Its counts start again, and there's no route to turn it on: only new samples clearing the threshold can.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/review/auto-commit/{combination}/disable", tag = "review", params(("id" = String, Path), ("combination" = String, Path)), request_body = memex_api::DisableBody, responses((status = 200, body = memex_api::AutoCommitDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn disable_auto_commit<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, combination)): Path<(String, String)>,
    parsed: Result<Json<memex_api::DisableBody>, JsonRejection>,
) -> Result<Json<memex_api::AutoCommitDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let entry = authed(server, headers, &rid, move |s, c| {
        s.disable_auto_commit(c, &VaultId(id), &ObjectId(combination), b.reason)
    })
    .await?;
    Ok(Json(entry_dto(entry)))
}
