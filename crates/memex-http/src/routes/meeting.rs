//! Routes for recordings: meeting metadata, the full transcript, media type and duration, and the bytes to play.
use super::{App, RequestId, authed, body, parse_range};
use crate::browser;
use crate::error::ApiError;
use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use memex_api::ErrorBody;
use memex_core::{Server, ServerEdges};
use memex_types::{ObjectId, VaultId};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(get_meeting, put_meeting, link_participant, get_transcript, get_media, stream_media),
    components(schemas(
        memex_api::ParticipantDto,
        memex_api::MeetingDto,
        memex_api::ParticipantInput,
        memex_api::MeetingBody,
        memex_api::ParticipantLinkBody,
        memex_api::MeetingProposalResult,
        memex_api::MediaInfoDto,
        memex_api::TranscriptWordDto,
        memex_api::TranscriptPartDto,
        memex_api::TranscriptDto
    ))
)]
pub(super) struct MeetingDoc;

pub(super) fn add_routes<E>(api: Router<App<E>>) -> Router<App<E>>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    api.route("/vaults/{id}/sources/{sid}/meeting", get(get_meeting::<E>).put(put_meeting::<E>))
        .route("/vaults/{id}/sources/{sid}/meeting/participants/{pid}/link", post(link_participant::<E>))
        .route("/vaults/{id}/sources/{sid}/transcript", get(get_transcript::<E>))
        .route("/vaults/{id}/sources/{sid}/media", get(get_media::<E>))
        .route("/vaults/{id}/sources/{sid}/stream", get(stream_media::<E>))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources/{sid}/meeting", tag = "meetings", params(("id" = String, Path), ("sid" = String, Path)), responses((status = 200, body = memex_api::MeetingDto), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn get_meeting<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
) -> Result<Json<memex_api::MeetingDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let m = authed(server, headers, &rid, move |s, c| s.meeting(c, &VaultId(id), &ObjectId(sid))).await?;
    Ok(Json(m.into()))
}

#[utoipa::path(put, path = "/api/v1/vaults/{id}/sources/{sid}/meeting", tag = "meetings", params(("id" = String, Path), ("sid" = String, Path)), request_body = memex_api::MeetingBody, responses((status = 200, body = memex_api::MeetingProposalResult, description = "Proposed. A person's commits at once and an agent's waits for review."), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
async fn put_meeting<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
    parsed: Result<Json<memex_api::MeetingBody>, JsonRejection>,
) -> Result<Json<memex_api::MeetingProposalResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let (meta, base, key) = body(parsed, &rid)?.meta();
    let source_id = sid.clone();
    let out = authed(server, headers, &rid, move |s, c| {
        s.propose_meeting(c, &VaultId(id), &ObjectId(sid), meta, base, key)
    })
    .await?;
    Ok(Json(memex_api::MeetingProposalResult {
        source_id,
        result: memex_api::proposal_result(out),
    }))
}

#[utoipa::path(post, path = "/api/v1/vaults/{id}/sources/{sid}/meeting/participants/{pid}/link", tag = "meetings", params(("id" = String, Path), ("sid" = String, Path), ("pid" = String, Path)), request_body = memex_api::ParticipantLinkBody, responses((status = 200, body = memex_api::MeetingProposalResult, description = "Proposed. A person's commits at once and an agent's waits for review."), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn link_participant<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid, pid)): Path<(String, String, String)>,
    parsed: Result<Json<memex_api::ParticipantLinkBody>, JsonRejection>,
) -> Result<Json<memex_api::MeetingProposalResult>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = body(parsed, &rid)?;
    let source_id = sid.clone();
    let out = authed(server, headers, &rid, move |s, c| {
        s.propose_participant_link(c, &VaultId(id), &ObjectId(sid), &pid, &ObjectId(b.entity_id), b.rationale, b.idempotency_key)
    })
    .await?;
    Ok(Json(memex_api::MeetingProposalResult {
        source_id,
        result: memex_api::proposal_result(out),
    }))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources/{sid}/transcript", tag = "meetings", params(("id" = String, Path), ("sid" = String, Path), memex_api::TranscriptQuery), responses((status = 200, body = memex_api::TranscriptDto), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody, description = "Not transcribed yet")))]
async fn get_transcript<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
    Query(q): Query<memex_api::TranscriptQuery>,
) -> Result<Json<memex_api::TranscriptDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let (from, to, after, limit) = q.validated().map_err(|m| ApiError::new(StatusCode::BAD_REQUEST, "invalid", m, &rid.0))?;
    let with_words = q.words.unwrap_or(false);
    let page = authed(server, headers, &rid, move |s, c| {
        s.transcript(c, &VaultId(id), &ObjectId(sid), from, to, after, limit)
    })
    .await?;
    Ok(Json(memex_api::transcript_dto(page, with_words)))
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources/{sid}/media", tag = "meetings", params(("id" = String, Path), ("sid" = String, Path)), responses((status = 200, body = memex_api::MediaInfoDto), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody)))]
async fn get_media<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
) -> Result<Json<memex_api::MediaInfoDto>, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let info = authed(server, headers, &rid, move |s, c| s.media_info(c, &VaultId(id), &ObjectId(sid))).await?;
    Ok(Json(info.into()))
}

/// Whether a Range header asks to start past the end, which is the one case that isn't just answered with the whole file. Pure.
pub(super) fn past_the_end(value: &str, size: u64) -> bool {
    let Some(spec) = value.strip_prefix("bytes=") else { return false };
    let Some((a, _)) = spec.split_once('-') else { return false };
    a.parse::<u64>().is_ok_and(|start| start >= size)
}

#[utoipa::path(get, path = "/api/v1/vaults/{id}/sources/{sid}/stream", tag = "meetings", params(("id" = String, Path), ("sid" = String, Path)), responses((status = 200, description = "The original recording, as it was uploaded, to play"), (status = 206, description = "Part of it, for a Range request. A player seeks to a cited time with these."), (status = 400, body = ErrorBody), (status = 404, body = ErrorBody), (status = 416, description = "The range starts past the end")))]
async fn stream_media<E>(
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
    let (vault, source) = (VaultId(id), ObjectId(sid));
    let (auth_headers, stream_headers) = (headers.clone(), headers);
    let (read_vault, read_source) = (vault.clone(), source.clone());
    let row = authed(server.clone(), auth_headers, &rid, move |s, c| s.recording_row(c, &read_vault, &read_source)).await?;
    if range.as_deref().is_some_and(|r| past_the_end(r, row.size)) {
        let mut res = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
        if let Ok(v) = HeaderValue::from_str(&format!("bytes */{}", row.size)) {
            res.headers_mut().insert(header::CONTENT_RANGE, v);
        }
        return Ok(res);
    }
    let part = range.as_deref().and_then(|r| parse_range(r, row.size));
    let (offset, len) = part.map_or((0, row.size), |(a, b)| (a, b - a + 1));
    // The channel is bounded, so a slow player slows the read and memory stays flat, whatever the file's size.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(8);
    tokio::task::spawn_blocking(move || {
        let caller = match browser::caller_from(&server, &stream_headers) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.blocking_send(Err(std::io::Error::other(e.to_string())));
                return;
            }
        };
        let result = server.stream_recording(&caller, &vault, &source, offset, len, &mut |chunk| {
            tx.blocking_send(Ok(axum::body::Bytes::copy_from_slice(chunk))).is_ok()
        });
        // Dropping the sender after an error ends the body short of its length, which a client sees as a failure.
        if let Err(e) = result {
            let _ = tx.blocking_send(Err(std::io::Error::other(e.to_string())));
        }
    });
    let body = axum::body::Body::from_stream(futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|item| (item, rx)) }));
    let mut res = (StatusCode::OK, body).into_response();
    let h = res.headers_mut();
    // Our own sniffed type, from a list of recordings. It never comes from the client, so nothing here can run as a page, and the sandbox says so again.
    if let Ok(v) = HeaderValue::from_str(&row.media_type) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(browser::DOWNLOAD_CSP));
    if let Ok(v) = HeaderValue::from_str(&len.to_string()) {
        h.insert(header::CONTENT_LENGTH, v);
    }
    if let Some((a, b)) = part {
        if let Ok(v) = HeaderValue::from_str(&format!("bytes {a}-{b}/{}", row.size)) {
            h.insert(header::CONTENT_RANGE, v);
        }
        *res.status_mut() = StatusCode::PARTIAL_CONTENT;
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_range_starting_past_the_end_is_unsatisfiable() {
        assert!(past_the_end("bytes=100-", 100));
        assert!(past_the_end("bytes=500-600", 100));
        assert!(!past_the_end("bytes=99-", 100));
        assert!(!past_the_end("bytes=-5", 100));
        assert!(!past_the_end("items=500-", 100));
    }
}
