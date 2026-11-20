//! Routes for operating the server (spec 039): operations on a vault as jobs, imports, effective settings, status that answers with Postgres or the LLM down, LLM usage, MCP's connection details and clearing the caches. Every route needs a vault admin or a server admin.
use super::{App, BodyReader, RequestId, authed, body, list};
use crate::browser;
use crate::error::ApiError;
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use memex_api::{ErrorBody, ImportDto, JobDto, JobPage, JobProgressDto, ListParams, ReachDto, page_of};
use memex_core::{ArchiveStore, Caches, Caller, Clock, Operation, RecentAdmins, RestoreTarget, Server, ServerEdges, ServerError};
use memex_types::{IndexName, ObjectId, OperationInput, VaultId};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use utoipa::OpenApi;

/// How long the status route gives each check, so the whole answer comes within 2 s whatever is down.
const AUTH_WAIT: Duration = Duration::from_millis(700);
const PROBE_WAIT: Duration = Duration::from_millis(900);

/// What the composition root knows and the routes show.
pub struct AdminConfig<A> {
    /// Export archives and imports waiting to be restored.
    pub archives: Arc<A>,
    /// `Settings::effective`, with every secret already left out.
    pub settings: Value,
    pub prices: Vec<memex_core::ModelPrice>,
    /// `host:port` of the LLM's endpoint, when one is set up.
    pub llm_endpoint: Option<String>,
    pub models: Vec<memex_api::ModelStatusDto>,
    pub mcp: memex_api::McpInfoDto,
    /// The process's resident memory in bytes, where the platform says.
    pub memory: fn() -> Option<u64>,
    /// Job handlers left running past their ceiling.
    pub abandoned: fn() -> usize,
}

struct AdminState<A> {
    config: AdminConfig<A>,
    recent: RecentAdmins,
    imports: Mutex<BTreeMap<String, ImportDto>>,
    /// Set while a Postgres check is out, so a hung one isn't joined by another every time someone asks.
    pg_probe_busy: Arc<AtomicBool>,
}

type Admin<A> = Arc<AdminState<A>>;

#[derive(OpenApi)]
#[openapi(
    paths(
        start_operation,
        list_operations,
        get_operation,
        operation_archive,
        start_import,
        get_import,
        settings,
        system_status,
        usage,
        mcp_info,
        clear_caches
    ),
    components(schemas(
        memex_api::OperationBody,
        memex_api::ImportDto,
        memex_api::JobProgressDto,
        memex_api::ReachDto,
        memex_api::ModelStatusDto,
        memex_api::SystemStatusDto,
        memex_api::ClearedCachesDto,
        memex_api::UsageRowDto,
        memex_api::UsageDto,
        memex_api::McpToolDto,
        memex_api::McpInfoDto
    ))
)]
pub(super) struct AdminDoc;

/// The admin routes. The composition root merges them in, as only it knows where archives live and what the settings are.
pub fn admin_routes<E, A>(config: AdminConfig<A>) -> Router<App<E>>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
    A: ArchiveStore + 'static,
{
    let state: Admin<A> = Arc::new(AdminState {
        config,
        recent: RecentAdmins::default(),
        imports: Mutex::new(BTreeMap::new()),
        pg_probe_busy: Arc::new(AtomicBool::new(false)),
    });
    Router::new()
        .route("/api/v1/vaults/{id}/operations", post(start_operation::<E>).get(list_operations::<E>))
        .route("/api/v1/vaults/{id}/operations/{jid}", get(get_operation::<E>))
        .route("/api/v1/vaults/{id}/operations/{jid}/archive", get(operation_archive::<E, A>))
        .route("/api/v1/imports", post(start_import::<E, A>).layer(DefaultBodyLimit::disable()))
        .route("/api/v1/imports/{iid}", get(get_import::<E, A>))
        .route("/api/v1/system/settings", get(settings::<E, A>))
        .route("/api/v1/system/status", get(system_status::<E, A>))
        .route("/api/v1/system/usage", get(usage::<E, A>))
        .route("/api/v1/system/mcp", get(mcp_info::<E, A>))
        .route("/api/v1/caches/clear", post(clear_caches::<E>))
        .layer(Extension(state))
}

fn bad(rid: &RequestId, message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid", message, &rid.0)
}

fn index_of(index: Option<&str>, rid: &RequestId) -> Result<IndexName, ApiError> {
    match index {
        Some("keyword") => Ok(IndexName::Keyword),
        Some("vector") => Ok(IndexName::Vector),
        _ => Err(bad(rid, "index is keyword or vector")),
    }
}

fn operation_input(b: memex_api::OperationBody, default_grace_s: u64, rid: &RequestId) -> Result<OperationInput, ApiError> {
    Ok(match b.op.as_str() {
        "reindex" => OperationInput::Reindex {
            index: index_of(b.index.as_deref(), rid)?,
        },
        "rollback" => OperationInput::Rollback {
            index: index_of(b.index.as_deref(), rid)?,
        },
        "reconcile" => OperationInput::Reconcile {
            at: b.at,
            deep: b.deep.unwrap_or(false),
            repair: b.repair.unwrap_or(false),
        },
        "export" => OperationInput::Export,
        "sweep" => OperationInput::Sweep {
            grace_s: b.grace_s.unwrap_or(default_grace_s),
        },
        "compact" => OperationInput::Compact,
        _ => return Err(bad(rid, "op is reindex, rollback, reconcile, export, sweep or compact")),
    })
}

/// Starts an operation on the vault as a job, and answers at once with the job. Its progress and, once done, its report are on the job. One of the same kind that's still queued or running is refused with 409, as is a rollback during a reindex of the same index. A vault admin's.
#[utoipa::path(post, path = "/api/v1/vaults/{id}/operations", tag = "operations", params(("id" = String, Path)), request_body = memex_api::OperationBody, responses((status = 202, body = JobDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn start_operation<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    parsed: Result<Json<memex_api::OperationBody>, JsonRejection>,
) -> Result<(StatusCode, Json<JobDto>), ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let input = operation_input(b, server.sweep_settings().grace_s, &rid)?;
    let job = authed(server, headers, &rid, move |s, c| s.start_operation(c, &VaultId(id), input)).await?;
    Ok((StatusCode::ACCEPTED, Json(job.into())))
}

/// The vault's operations, oldest first. A vault admin's.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/operations", tag = "operations", params(("id" = String, Path), ListParams), responses((status = 200, body = JobPage), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn list_operations<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> Result<Json<JobPage>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let l = list(params, &rid)?;
    let rows = authed(server, headers, &rid, move |s, c| {
        s.operations(c, &VaultId(id), l.cursor.map(ObjectId).as_ref(), l.limit + 1)
    })
    .await?;
    let (items, next_cursor) = page_of(rows, l.limit, |j| j.header.id.0.clone());
    Ok(Json(JobPage {
        items: items.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

/// One operation: its state, its progress, and its report once done. A vault admin's.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/operations/{jid}", tag = "operations", params(("id" = String, Path), ("jid" = String, Path)), responses((status = 200, body = JobDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn get_operation<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, jid)): Path<(String, String)>,
) -> Result<Json<JobDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let job = authed(server, headers, &rid, move |s, c| s.operation(c, &VaultId(id), &ObjectId(jid))).await?;
    Ok(Json(job.into()))
}

/// The archive a finished export wrote, the same format as `memexvault backup`, which `memexvault restore` and `POST /api/v1/imports` read. A vault admin's.
#[utoipa::path(get, path = "/api/v1/vaults/{id}/operations/{jid}/archive", tag = "operations", params(("id" = String, Path), ("jid" = String, Path)), responses((status = 200, content_type = "application/octet-stream", body = Vec<u8>), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn operation_archive<E, A>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    Extension(admin): Extension<Admin<A>>,
    headers: HeaderMap,
    Path((id, jid)): Path<(String, String)>,
) -> Result<Response, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
    A: ArchiveStore + 'static,
{
    let name = format!("{id}-{jid}.memex");
    let archives = admin.config.archives.clone();
    let (size, mut read) = authed(server, headers, &rid, move |s, c| {
        s.operation_archive(c, &VaultId(id), &ObjectId(jid), &*archives)
    })
    .await?;
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(4);
    tokio::task::spawn_blocking(move || {
        loop {
            let mut buf = vec![0u8; 256 * 1024];
            match read.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    buf.truncate(n);
                    if tx.blocking_send(Ok(buf)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.blocking_send(Err(e));
                    break;
                }
            }
        }
    });
    let stream = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|item| (item, rx)) });
    let mut res = Response::new(Body::from_stream(stream));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(browser::DOWNLOAD_CSP));
    let safe: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{safe}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(res)
}

use std::io::Read as _;

fn import_name(id: &str) -> String {
    format!("imports/{id}.memex")
}

fn now<E: ServerEdges>(server: &Server<E>) -> i64 {
    server.engine().clock().now_ms()
}

/// Restores an archive, the body, into a vault, as `memexvault restore` does. It's checked whole before anything is written, and a failed restore removes what it made. Over an existing vault only with `replace`, and refused at once otherwise. Answers once the archive is received, and the restore goes on in the background. A server admin's.
#[utoipa::path(post, path = "/api/v1/imports", tag = "operations", params(memex_api::ImportQuery), request_body(content = Vec<u8>, content_type = "application/octet-stream"), responses((status = 202, body = ImportDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn start_import<E, A>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    Extension(admin): Extension<Admin<A>>,
    headers: HeaderMap,
    Query(q): Query<memex_api::ImportQuery>,
    req_body: Body,
) -> Result<(StatusCode, Json<ImportDto>), ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
    A: ArchiveStore + 'static,
{
    let target = match (q.new_id.unwrap_or(false), q.replace.unwrap_or(false)) {
        (true, true) => return Err(bad(&rid, "new_id and replace can't go together")),
        (true, false) => RestoreTarget::NewId,
        (false, true) => RestoreTarget::Replace,
        (false, false) => RestoreTarget::Original,
    };
    // Who's asking comes first, so nobody else's upload reaches the disk.
    let caller = authed(server.clone(), headers, &rid, |s, c| {
        s.authorize_op(c, Operation::VaultCreate)?;
        Ok(c.clone())
    })
    .await?;
    let id = server.new_request_id();
    let name = import_name(&id);
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(async move {
        use futures_util::StreamExt;
        let mut stream = req_body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            if tx.send(chunk.map_err(std::io::Error::other)).await.is_err() {
                break;
            }
        }
    });
    let (archives, n2, t2) = (admin.config.archives.clone(), name.clone(), target.clone());
    let s2 = server.clone();
    let received = tokio::task::spawn_blocking(move || -> Result<VaultId, ServerError> {
        let mut reader = BodyReader {
            rx,
            current: axum::body::Bytes::new(),
        };
        archives.put(&n2, &mut |w| {
            std::io::copy(&mut reader, w).map_err(|e| ServerError::Invalid(format!("the archive didn't arrive whole: {e}")))?;
            Ok(())
        })?;
        let checked = (|| {
            let (_, mut read) = archives.open(&n2)?.ok_or(ServerError::NotFound)?;
            s2.check_restore(&mut read, &t2)
        })();
        if checked.is_err() {
            let _ = archives.remove(&n2);
        }
        checked
    })
    .await
    .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "the request failed", &rid.0))?;
    received.map_err(|e| ApiError::from_server(e, &rid.0))?;
    let started = ImportDto {
        id: id.clone(),
        state: "running".into(),
        progress: JobProgressDto {
            step: 1,
            of: 2,
            label: "checking the archive and restoring it".into(),
        },
        started_at_ms: now(&server),
        finished_at_ms: None,
        error: None,
        report: None,
    };
    admin.imports.lock().expect("lock").insert(id.clone(), started.clone());
    let restore_name = q.name;
    tokio::task::spawn_blocking(move || {
        let archives = admin.config.archives.clone();
        let mut open = || -> std::io::Result<Box<dyn std::io::Read>> {
            match archives.open(&name) {
                Ok(Some((_, r))) => Ok(r as Box<dyn std::io::Read>),
                Ok(None) => Err(std::io::Error::new(std::io::ErrorKind::NotFound, "the uploaded archive is gone")),
                Err(e) => Err(std::io::Error::other(e.to_string())),
            }
        };
        let result = server.restore_vault(&caller.principal.id, &mut open, target, restore_name.as_deref());
        let _ = archives.remove(&name);
        let finished = now(&server);
        let mut imports = admin.imports.lock().expect("lock");
        if let Some(entry) = imports.get_mut(&id) {
            entry.finished_at_ms = Some(finished);
            match result {
                Ok((row, summary)) => {
                    entry.state = "done".into();
                    entry.progress = JobProgressDto {
                        step: 2,
                        of: 2,
                        label: "restored. Its indexes build in the background".into(),
                    };
                    entry.report = Some(json!({
                        "vault_id": row.id.0,
                        "name": row.name,
                        "events": summary.events,
                        "blobs": summary.blobs,
                        "embeddings": summary.embeddings,
                        "sha256": summary.sha256,
                    }));
                }
                Err(e) => {
                    entry.state = "failed".into();
                    entry.error = Some(memex_api::error_message(&e, &id));
                }
            }
        }
    });
    Ok((StatusCode::ACCEPTED, Json(started)))
}

/// An import's progress and, once done, the vault it made. Held in memory, so gone after a restart. A server admin's.
#[utoipa::path(get, path = "/api/v1/imports/{iid}", tag = "operations", params(("iid" = String, Path)), responses((status = 200, body = ImportDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn get_import<E, A>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    Extension(admin): Extension<Admin<A>>,
    headers: HeaderMap,
    Path(iid): Path<String>,
) -> Result<Json<ImportDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
    A: ArchiveStore + 'static,
{
    authed(server, headers, &rid, |s, c| s.authorize_op(c, Operation::VaultCreate)).await?;
    let found = admin.imports.lock().expect("lock").get(&iid).cloned();
    found.map(Json).ok_or_else(|| ApiError::from_server(ServerError::NotFound, &rid.0))
}

/// The settings the server runs with, read-only. Secrets are left out: the database URL keeps its host and database only, and keys and tokens say only whether they're set. A server admin's.
#[utoipa::path(get, path = "/api/v1/system/settings", tag = "operations", responses((status = 200, body = Object), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody)))]
async fn settings<E, A>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    Extension(admin): Extension<Admin<A>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
    A: ArchiveStore + 'static,
{
    authed(server, headers, &rid, |s, c| s.authorize_op(c, Operation::ServerStatus)).await?;
    Ok(Json(admin.config.settings.clone()))
}

fn millis(d: Duration) -> u64 {
    d.as_millis().min(u128::from(u64::MAX)) as u64
}

async fn probe_postgres<E, A>(server: App<E>, admin: &Admin<A>) -> ReachDto
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let down = |error: String| ReachDto {
        configured: true,
        reachable: false,
        latency_ms: None,
        error: Some(error),
    };
    if admin.pg_probe_busy.swap(true, Ordering::AcqRel) {
        return down("an earlier check is still waiting for an answer".into());
    }
    let busy = admin.pg_probe_busy.clone();
    let start = tokio::time::Instant::now();
    let answer = tokio::time::timeout(
        PROBE_WAIT,
        tokio::task::spawn_blocking(move || {
            let ok = server.healthy();
            busy.store(false, Ordering::Release);
            ok
        }),
    )
    .await;
    match answer {
        Ok(Ok(true)) => ReachDto {
            configured: true,
            reachable: true,
            latency_ms: Some(millis(start.elapsed())),
            error: None,
        },
        Ok(Ok(false)) => down("the registry couldn't be read".into()),
        Ok(Err(_)) => down("the check failed".into()),
        Err(_) => down(format!("no answer within {} ms", millis(PROBE_WAIT))),
    }
}

/// Whether a connection opens to the LLM's endpoint. It sends nothing, so it costs nothing.
async fn probe_llm(endpoint: Option<String>) -> ReachDto {
    let Some(endpoint) = endpoint else {
        return ReachDto {
            configured: false,
            reachable: false,
            latency_ms: None,
            error: None,
        };
    };
    let start = tokio::time::Instant::now();
    let down = |error: String| ReachDto {
        configured: true,
        reachable: false,
        latency_ms: None,
        error: Some(error),
    };
    match tokio::time::timeout(PROBE_WAIT, tokio::net::TcpStream::connect(endpoint.as_str())).await {
        Ok(Ok(_)) => ReachDto {
            configured: true,
            reachable: true,
            latency_ms: Some(millis(start.elapsed())),
            error: None,
        },
        Ok(Err(e)) => down(format!("{endpoint}: {e}")),
        Err(_) => down(format!("{endpoint}: no connection within {} ms", millis(PROBE_WAIT))),
    }
}

/// Postgres and the LLM, memory, caches, and each vault's index lag, queue depth and sizes, with the backends, models and thresholds. It answers within 2 s even with Postgres or the LLM down. While Postgres is down a server admin's credential checked in the last 5 minutes still works here, and only here. A server admin's.
#[utoipa::path(get, path = "/api/v1/system/status", tag = "operations", responses((status = 200, body = memex_api::SystemStatusDto), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
async fn system_status<E, A>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    Extension(admin): Extension<Admin<A>>,
    headers: HeaderMap,
) -> Result<Json<memex_api::SystemStatusDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
    A: ArchiveStore + 'static,
{
    let credential = browser::credential(&headers).map(str::to_string);
    let s = server.clone();
    let checked = tokio::time::timeout(
        AUTH_WAIT,
        tokio::task::spawn_blocking(move || -> Result<Caller, ServerError> {
            let caller = browser::caller_from(&s, &headers)?;
            s.authorize_op(&caller, Operation::ServerStatus)?;
            Ok(caller)
        }),
    )
    .await;
    let unreachable = |message: String| ApiError::from_server(ServerError::Unavailable(message), &rid.0);
    let (caller, from_memory) = match checked {
        Ok(Ok(Ok(caller))) => {
            if let Some(c) = &credential {
                admin.recent.remember(c, &caller, now(&server));
            }
            (caller, false)
        }
        // Only when Postgres couldn't answer. A refusal is final.
        Ok(Ok(Err(ServerError::Unavailable(m)))) => match credential.as_deref().and_then(|c| admin.recent.recall(c, now(&server))) {
            Some(caller) => (caller, true),
            None => return Err(unreachable(m)),
        },
        Ok(Ok(Err(e))) => return Err(ApiError::from_server(e, &rid.0)),
        Ok(Err(_)) => return Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "the request failed", &rid.0)),
        Err(_) => match credential.as_deref().and_then(|c| admin.recent.recall(c, now(&server))) {
            Some(caller) => (caller, true),
            None => return Err(unreachable("Postgres didn't answer in time to check the token".into())),
        },
    };
    let s = server.clone();
    let vaults = async move {
        match tokio::time::timeout(PROBE_WAIT, tokio::task::spawn_blocking(move || s.status(&caller))).await {
            Ok(Ok(Ok(status))) => Ok(status.vaults),
            Ok(Ok(Err(e))) => Err(memex_api::error_message(&e, "status")),
            Ok(Err(_)) => Err("the check failed".to_string()),
            Err(_) => Err(format!("no answer within {} ms", millis(PROBE_WAIT))),
        }
    };
    let (postgres, llm, vaults) = tokio::join!(probe_postgres(server.clone(), &admin), probe_llm(admin.config.llm_endpoint.clone()), vaults);
    let settings = &admin.config.settings;
    let (vaults, vaults_error) = match vaults {
        Ok(v) => (Some(v.into_iter().map(Into::into).collect()), None),
        Err(e) => (None, Some(e)),
    };
    Ok(Json(memex_api::SystemStatusDto {
        version: env!("CARGO_PKG_VERSION").into(),
        postgres,
        llm,
        memory_rss_bytes: (admin.config.memory)(),
        abandoned_handlers: (admin.config.abandoned)() as u64,
        backends: settings.get("backends").cloned().unwrap_or(Value::Null),
        models: admin.config.models.clone(),
        thresholds: settings.get("thresholds").cloned().unwrap_or(Value::Null),
        caches: Caches::stats(server.caches()).into_iter().map(Into::into).collect(),
        vaults,
        vaults_error,
        caller_from_memory: from_memory,
    }))
}

/// LLM tokens by agent, model, vault and UTC day, summed from Derivations, with cost only for a model `MEMEX_LLM_PRICES` gives a price. A server admin's.
#[utoipa::path(get, path = "/api/v1/system/usage", tag = "operations", params(memex_api::UsageQuery), responses((status = 200, body = memex_api::UsageDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody)))]
async fn usage<E, A>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    Extension(admin): Extension<Admin<A>>,
    headers: HeaderMap,
    Query(q): Query<memex_api::UsageQuery>,
) -> Result<Json<memex_api::UsageDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
    A: ArchiveStore + 'static,
{
    let day = |text: Option<String>, default: i64, end: bool| -> Result<i64, ApiError> {
        match text {
            None => Ok(default),
            Some(t) => memex_core::parse_utc_day(&t)
                .map(|ms| if end { ms + 86_400_000 } else { ms })
                .ok_or_else(|| bad(&rid, format!("'{t}' isn't a day as YYYY-MM-DD"))),
        }
    };
    let (from, to) = (day(q.from, i64::MIN, false)?, day(q.to, i64::MAX, true)?);
    let prices = admin.config.prices.clone();
    let report = authed(server, headers, &rid, move |s, c| s.usage(c, from, to, &prices)).await?;
    Ok(Json(report.into()))
}

/// How to connect an MCP client: the endpoint, transports, protocol versions and the tools. A server admin's.
#[utoipa::path(get, path = "/api/v1/system/mcp", tag = "operations", responses((status = 200, body = memex_api::McpInfoDto), (status = 403, body = ErrorBody)))]
async fn mcp_info<E, A>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    Extension(admin): Extension<Admin<A>>,
    headers: HeaderMap,
) -> Result<Json<memex_api::McpInfoDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
    A: ArchiveStore + 'static,
{
    authed(server, headers, &rid, |s, c| s.authorize_op(c, Operation::ServerStatus)).await?;
    Ok(Json(admin.config.mcp.clone()))
}

/// Drops every cached entry, in RAM and on disk, for every vault. The counters stay. A server admin's.
#[utoipa::path(post, path = "/api/v1/caches/clear", tag = "operations", responses((status = 200, body = memex_api::ClearedCachesDto), (status = 403, body = ErrorBody)))]
async fn clear_caches<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
) -> Result<Json<memex_api::ClearedCachesDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let (vaults, caches) = authed(server, headers, &rid, |s, c| Ok((s.clear_caches(c)?, Caches::stats(s.caches())))).await?;
    Ok(Json(memex_api::ClearedCachesDto {
        vaults: vaults as u64,
        caches: caches.into_iter().map(Into::into).collect(),
    }))
}
