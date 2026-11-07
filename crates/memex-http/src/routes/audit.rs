//! Routes for the audit agent: runs and what they found, the schedule and budget, and planted errors in a test vault.
use super::{App, RequestId, authed, body, list};
use crate::error::ApiError;
use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use memex_api::{ErrorBody, ListParams, page_of};
use memex_core::{AuditRunDetail, Server, ServerEdges, TriggerAudit};
use memex_types::{JobState, ObjectId, PlantedKind, VaultId};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(list_runs, trigger_run, get_run, run_checks, get_schedule, put_schedule, get_budget, get_recall, mark_test_vault, plant_errors),
    components(schemas(
        memex_api::TriggerAuditBody,
        memex_api::VaultFindingDto,
        memex_api::AuditRunDto,
        memex_api::AuditRunPage,
        memex_api::AuditRunDetailDto,
        memex_api::AuditCheckResultDto,
        memex_api::AuditCombinationDto,
        memex_api::AuditCheckDto,
        memex_api::AuditCheckPage,
        memex_api::AuditScheduleBody,
        memex_api::AuditScheduleDto,
        memex_api::AuditBudgetDto,
        memex_api::PlantBody,
        memex_api::PlantedCountDto,
        memex_api::PlantedDto,
        memex_api::KindRecallDto,
        memex_api::GroupRecallDto,
        memex_api::RecallReportDto
    ))
)]
pub(super) struct AuditDoc;

pub(super) fn add_routes<E>(api: Router<App<E>>) -> Router<App<E>>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    api.route("/vaults/{id}/audit/runs", get(list_runs::<E>).post(trigger_run::<E>))
        .route("/vaults/{id}/audit/runs/{run}", get(get_run::<E>))
        .route("/vaults/{id}/audit/runs/{run}/checks", get(run_checks::<E>))
        .route("/vaults/{id}/audit/schedule", get(get_schedule::<E>).put(put_schedule::<E>))
        .route("/vaults/{id}/audit/budget", get(get_budget::<E>))
        .route("/vaults/{id}/audit/recall", get(get_recall::<E>))
        .route("/vaults/{id}/audit/test-vault", post(mark_test_vault::<E>))
        .route("/vaults/{id}/audit/planted", post(plant_errors::<E>))
}

fn invalid(message: impl Into<String>, rid: &RequestId) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid", message, &rid.0)
}

fn seed_of(text: &str, rid: &RequestId) -> Result<u64, ApiError> {
    text.parse().map_err(|_| invalid("a seed is a whole number, as text", rid))
}

fn run_dto(d: AuditRunDetail) -> memex_api::AuditRunDto {
    memex_api::AuditRunDto::new(d.run, d.job_state == Some(JobState::Dead) || d.job_state == Some(JobState::Cancelled))
}

/// Runs in creation order, with their seeds, what each found and what it spent.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/audit/runs", tag = "audit", params(("id" = String, Path), ListParams), responses((status = 200, body = memex_api::AuditRunPage), (status = 404, body = ErrorBody)))]
async fn list_runs<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<memex_api::AuditRunPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.list_audit_runs(c, &VaultId(id), l.cursor.map(ObjectId).as_ref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, l.limit, |r| r.run.header.id.0.clone());
    Ok(Json(memex_api::AuditRunPage {
        items: items.into_iter().map(run_dto).collect(),
        next_cursor,
    }))
}

/// Starts a run now. A seed makes it repeatable: the same seed on the same state samples the same claims as any other run with that seed, scheduled or not. It may spend tokens, so it needs admin scope.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/audit/runs", tag = "audit", params(("id" = String, Path)), request_body = memex_api::TriggerAuditBody, responses((status = 202, body = memex_api::AuditRunDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn trigger_run<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    parsed: Result<Json<memex_api::TriggerAuditBody>, JsonRejection>,
) -> Result<(StatusCode, Json<memex_api::AuditRunDto>), ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let seed = b.seed.as_deref().map(|t| seed_of(t, &rid)).transpose()?;
    let wanted = TriggerAudit {
        seed,
        sample_size: b.sample_size,
        run_token_budget: b.run_token_budget,
    };
    let run = authed(server, headers, &rid, move |s, c| s.trigger_audit(c, &VaultId(id), wanted)).await?;
    Ok((StatusCode::ACCEPTED, Json(memex_api::AuditRunDto::new(run, false))))
}

/// One run, with its seed and its sample in rank order.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/audit/runs/{run}", tag = "audit", params(("id" = String, Path), ("run" = String, Path)), responses((status = 200, body = memex_api::AuditRunDetailDto), (status = 404, body = ErrorBody)))]
async fn get_run<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, run)): Path<(String, String)>,
) -> Result<Json<memex_api::AuditRunDetailDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let (detail, checks) = authed(server, headers, &rid, move |s, c| {
        let (vault, run) = (VaultId(id), ObjectId(run));
        let detail = s.get_audit_run(c, &vault, &run)?;
        let checks = s.audit_run_checks(c, &vault, &run, false, None, memex_core::audit::MAX_SAMPLE as usize)?;
        Ok((detail, checks))
    })
    .await?;
    Ok(Json(memex_api::AuditRunDetailDto {
        run: run_dto(detail),
        sample: checks.into_iter().map(|c| c.claim_id.0).collect(),
    }))
}

/// What was found about each sampled claim, in rank order, with each check's result and the review item it led to. `findings=true` leaves out what passed.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/audit/runs/{run}/checks", tag = "audit", params(("id" = String, Path), ("run" = String, Path), ListParams, memex_api::AuditChecksQuery), responses((status = 200, body = memex_api::AuditCheckPage), (status = 404, body = ErrorBody)))]
async fn run_checks<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, run)): Path<(String, String)>,
    Query(params): Query<ListParams>,
    Query(query): Query<memex_api::AuditChecksQuery>,
) -> Result<Json<memex_api::AuditCheckPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let findings = query.findings.unwrap_or(false);
    let rows = authed(server, headers, &rid, move |s, c| {
        s.audit_run_checks(c, &VaultId(id), &ObjectId(run), findings, l.cursor.map(ObjectId).as_ref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, l.limit, |r| r.header.id.0.clone());
    Ok(Json(memex_api::AuditCheckPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/audit/schedule", tag = "audit", params(("id" = String, Path)), responses((status = 200, body = memex_api::AuditScheduleDto), (status = 404, body = ErrorBody)))]
async fn get_schedule<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<memex_api::AuditScheduleDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let (schedule, next, missed) = authed(server, headers, &rid, move |s, c| s.audit_schedule(c, &VaultId(id))).await?;
    Ok(Json(memex_api::AuditScheduleDto::new(schedule, next, missed)))
}

/// Replaces the schedule, which is off until someone turns it on. The first run comes one interval from now.
#[utoipa::path(put, path = "/api/v1/vaults/{id}/audit/schedule", tag = "audit", params(("id" = String, Path)), request_body = memex_api::AuditScheduleBody, responses((status = 200, body = memex_api::AuditScheduleDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn put_schedule<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    parsed: Result<Json<memex_api::AuditScheduleBody>, JsonRejection>,
) -> Result<Json<memex_api::AuditScheduleDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let (schedule, next, missed) = authed(server, headers, &rid, move |s, c| s.set_audit_schedule(c, &VaultId(id), b.into())).await?;
    Ok(Json(memex_api::AuditScheduleDto::new(schedule, next, missed)))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/audit/budget", tag = "audit", params(("id" = String, Path)), responses((status = 200, body = memex_api::AuditBudgetDto), (status = 404, body = ErrorBody)))]
async fn get_budget<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<memex_api::AuditBudgetDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = authed(server, headers, &rid, move |s, c| s.audit_budget(c, &VaultId(id))).await?;
    Ok(Json(memex_api::AuditBudgetDto {
        run_token_budget: b.run_token_budget,
        day_token_budget: b.day_token_budget,
        day: b.day,
        day_spent: b.day_spent,
        day_remaining: b.day_remaining,
    }))
}

/// What planted errors came to, per kind and in two groups: kinds the deterministic checks catch, and kinds only LLM checks can. Scripted answers say nothing about a model, so `target_met` stays null until a real model's run.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/audit/recall", tag = "audit", params(("id" = String, Path)), responses((status = 200, body = memex_api::RecallReportDto), (status = 404, body = ErrorBody)))]
async fn get_recall<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<memex_api::RecallReportDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let r = authed(server, headers, &rid, move |s, c| s.audit_recall(c, &VaultId(id))).await?;
    let group = |g: memex_core::audit::GroupRecall| memex_api::GroupRecallDto {
        planted: g.planted,
        audited: g.audited,
        caught: g.caught,
        recall: g.recall,
    };
    Ok(Json(memex_api::RecallReportDto {
        kinds: r
            .kinds
            .into_iter()
            .map(|k| memex_api::KindRecallDto {
                kind: k.kind.as_str().into(),
                needs_llm: k.needs_llm,
                planted: k.planted,
                audited: k.audited,
                caught: k.caught,
                recall: k.recall,
            })
            .collect(),
        deterministic: group(r.deterministic),
        llm_only: group(r.llm_only),
        overall_planted: r.overall_planted,
        overall_caught: r.overall_caught,
        overall_recall: r.overall_recall,
        llm_evidence: r.llm_evidence.as_str().into(),
        llm_models: r.llm_models,
        clean_audited: r.clean_audited,
        false_positives_deterministic: r.false_positives_deterministic,
        false_positives_llm: r.false_positives_llm,
        target: r.target,
        target_met: r.target_met,
        too_small: r.too_small,
    }))
}

/// Marks an empty vault as one errors can be planted in. It can't be undone.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/audit/test-vault", tag = "audit", params(("id" = String, Path)), responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn mark_test_vault<E>(State(server): State<App<E>>, Extension(rid): Extension<RequestId>, headers: HeaderMap, Path(id): Path<String>) -> Result<StatusCode, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    authed(server, headers, &rid, move |s, c| s.mark_audit_test_vault(c, &VaultId(id))).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Corrupts claims in a test vault and tags them, tagging what's left clean, to measure what audits catch. It's refused on any other vault.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/audit/planted", tag = "audit", params(("id" = String, Path)), request_body = memex_api::PlantBody, responses((status = 200, body = memex_api::PlantedDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn plant_errors<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    parsed: Result<Json<memex_api::PlantBody>, JsonRejection>,
) -> Result<Json<memex_api::PlantedDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let seed = seed_of(&b.seed, &rid)?;
    let kinds: Vec<PlantedKind> = match b.kinds {
        None => PlantedKind::ERRORS.to_vec(),
        Some(names) => names
            .iter()
            .map(|n| PlantedKind::parse(n).ok_or_else(|| invalid(format!("unknown kind {n}"), &rid)))
            .collect::<Result<_, _>>()?,
    };
    let per_kind = b.per_kind;
    let planted = authed(server, headers, &rid, move |s, c| s.plant_audit_errors(c, &VaultId(id), seed, per_kind, &kinds)).await?;
    Ok(Json(memex_api::PlantedDto {
        kinds: planted
            .into_iter()
            .map(|(k, n)| memex_api::PlantedCountDto {
                kind: k.as_str().into(),
                planted: n,
            })
            .collect(),
    }))
}
