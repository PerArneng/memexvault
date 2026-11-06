//! What browsers need beyond the API: session cookies, the CSRF rule, security headers and the embedded console.
use crate::error::ApiError;
use crate::routes::{App, RequestId};
use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Extension, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use memex_api::{ErrorBody, LoginBody, WhoAmI};
use memex_core::{Caller, Server, ServerEdges, ServerError};
use std::borrow::Cow;
use std::net::SocketAddr;

pub const SESSION_COOKIE: &str = "memex_session";
/// Pages from other sites can't add a custom header without a CORS preflight, and this server answers none.
pub const CSRF_HEADER: &str = "x-memex-csrf";
/// Everything that reads the cookie sits under this path.
const COOKIE_PATH: &str = "/api/v1";
const SESSION_ROUTE: &str = "/api/v1/session";

const CSP: &str = "default-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; object-src 'none'";
/// Original files are untrusted. If a browser ever renders one, it runs with no scripts and no origin.
pub(crate) const DOWNLOAD_CSP: &str = "default-src 'none'; frame-ancestors 'none'; sandbox";

/// Headers a route already set stay, so the download route can be stricter.
pub(crate) fn security_headers(h: &mut HeaderMap) {
    for (name, value) in [
        (header::CONTENT_SECURITY_POLICY, CSP),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::REFERRER_POLICY, "no-referrer"),
    ] {
        h.entry(name).or_insert(HeaderValue::from_static(value));
    }
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(SESSION_COOKIE)?.strip_prefix('='))
}

/// A bearer header decides alone when present, so a bad one never falls back to a cookie.
pub(crate) fn caller_from<E>(server: &Server<E>, headers: &HeaderMap) -> Result<Caller, ServerError>
where
    E: ServerEdges,
{
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        let token = value.to_str().ok().and_then(|v| v.strip_prefix("Bearer "));
        return server.authenticate(token.ok_or(ServerError::Unauthenticated)?);
    }
    server.authenticate_session(session_cookie(headers).ok_or(ServerError::Unauthenticated)?)
}

/// Refuses state-changing requests that a cookie would authorize unless they carry the custom header. Signing in counts too, so another site can't sign a browser in to its own account.
pub(crate) async fn csrf_guard(Extension(rid): Extension<RequestId>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    let needs_header = !safe && !headers.contains_key(header::AUTHORIZATION) && (session_cookie(headers).is_some() || req.uri().path() == SESSION_ROUTE);
    if needs_header && !headers.contains_key(CSRF_HEADER) {
        return ApiError::new(StatusCode::FORBIDDEN, "csrf", format!("this request needs the {CSRF_HEADER} header"), &rid.0).into_response();
    }
    next.run(req).await
}

fn set_cookie(res: &mut Response, value: &str, max_age_s: i64, secure: bool) {
    let secure = if secure { "; Secure" } else { "" };
    let cookie = format!("{SESSION_COOKIE}={value}; Path={COOKIE_PATH}; HttpOnly; SameSite=Strict; Max-Age={max_age_s}{secure}");
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        res.headers_mut().append(header::SET_COOKIE, v);
    }
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

/// The server itself speaks plain HTTP, so TLS means a proxy in front, and the proxy says so in this header. A client that forges it only gets a cookie it can't use over plain HTTP.
fn on_tls(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("https"))
}

#[utoipa::path(post, path = "/api/v1/session", tag = "server", request_body = LoginBody, responses((status = 201, body = WhoAmI, description = "Signed in. The session is in an HttpOnly cookie."), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody, description = "The x-memex-csrf header is missing"), (status = 429, body = ErrorBody, description = "Too many failed attempts")))]
pub(crate) async fn login<E>(
    State(server): State<App<E>>,
    Extension(rid): Extension<RequestId>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    parsed: Result<Json<LoginBody>, JsonRejection>,
) -> Result<Response, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let b = crate::routes::body(parsed, &rid)?;
    // The peer's address, not a forwarded one, which anyone could vary. Behind a proxy all clients therefore share a count.
    let client = peer.map_or_else(|| "unknown".to_string(), |Extension(ConnectInfo(addr))| addr.ip().to_string());
    let result = tokio::task::spawn_blocking(move || server.login(&b.token, &client)).await;
    let created = match result {
        Ok(r) => r.map_err(|e| ApiError::from_server(e, &rid.0))?,
        Err(_) => return Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "the request failed", &rid.0)),
    };
    let me = WhoAmI {
        principal: created.caller.principal.clone().into(),
        token_id: created.caller.token_id.clone(),
        grants: created.caller.grants.iter().cloned().map(Into::into).collect(),
    };
    let mut res = (StatusCode::CREATED, Json(me)).into_response();
    set_cookie(&mut res, &created.session_id, created.max_age_s, on_tls(&headers));
    Ok(res)
}

#[utoipa::path(delete, path = "/api/v1/session", tag = "server", responses((status = 204, description = "Signed out, or there was no session"), (status = 403, body = ErrorBody, description = "The x-memex-csrf header is missing")))]
pub(crate) async fn logout<E>(State(server): State<App<E>>, Extension(rid): Extension<RequestId>, headers: HeaderMap) -> Result<Response, ApiError>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    if let Some(id) = session_cookie(&headers).map(str::to_string) {
        let result = tokio::task::spawn_blocking(move || server.logout(&id)).await;
        match result {
            Ok(r) => r.map_err(|e| ApiError::from_server(e, &rid.0))?,
            Err(_) => return Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "the request failed", &rid.0)),
        }
    }
    let mut res = StatusCode::NO_CONTENT.into_response();
    set_cookie(&mut res, "", 0, on_tls(&headers));
    Ok(res)
}

/// Finds one of the console's files. `path` has no leading slash.
pub type ConsoleLookup = fn(&str) -> Option<Cow<'static, [u8]>>;

/// `allow_missing` lets the backend build without `web/` having been built, and `debug-embed` makes every profile read the embedded copy, never the disk.
#[derive(rust_embed::RustEmbed)]
#[folder = "../../web/apps/console/dist/"]
#[allow_missing = true]
struct Dist;

pub fn embedded_asset(path: &str) -> Option<Cow<'static, [u8]>> {
    Dist::get(path).map(|f| f.data)
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map_or("", |(_, ext)| ext) {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn console_file(path: &str, cache: &'static str, bytes: Cow<'static, [u8]>) -> Response {
    let mut res = (StatusCode::OK, bytes.into_owned()).into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(path)));
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    res
}

const NO_CONSOLE_PAGE: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>memexvault: no console</title></head><body><h1>This build has no console</h1><p>The API works. To add the console, run <code>pnpm install</code> and <code>pnpm build</code> in <code>web/</code>, then build the server again.</p></body></html>";

/// Everything no route matched. Unknown API paths get the API's error. Other GETs get a file, or `index.html` so the console's own routes work on reload. Paths only ever look up names in the embedded table, so `..` can't reach a file.
pub(crate) async fn console_or_not_found(console: ConsoleLookup, rid: RequestId, req: Request) -> Response {
    let path = req.uri().path();
    if path == "/api" || path.starts_with("/api/") || !matches!(*req.method(), Method::GET | Method::HEAD) {
        return ApiError::new(StatusCode::NOT_FOUND, "not_found", "not found", &rid.0).into_response();
    }
    let path = path.trim_start_matches('/');
    if !path.is_empty() && path != "index.html" {
        if let Some(bytes) = console(path) {
            // Vite puts a hash of the content in every name under assets/, so these never change.
            let cache = if path.starts_with("assets/") {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            };
            return console_file(path, cache, bytes);
        }
    }
    match console("index.html") {
        Some(bytes) => console_file("index.html", "no-store", bytes),
        None => {
            let mut res = (StatusCode::SERVICE_UNAVAILABLE, axum::response::Html(NO_CONSOLE_PAGE)).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            res
        }
    }
}
