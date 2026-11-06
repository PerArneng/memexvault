use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use memex_api::{ErrorBody, ErrorDetail};
use memex_core::ServerError;

pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    request_id: String,
    current: Option<serde_json::Value>,
}

impl ApiError {
    pub fn is_not_caught_up(&self) -> bool {
        self.code == "not_caught_up"
    }

    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>, request_id: &str) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            request_id: request_id.into(),
            current: None,
        }
    }

    pub fn from_server(e: ServerError, request_id: &str) -> Self {
        let (status, code) = match &e {
            ServerError::Unauthenticated => (StatusCode::UNAUTHORIZED, "unauthenticated"),
            ServerError::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            ServerError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            ServerError::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid"),
            ServerError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            ServerError::StaleVersion { .. } => (StatusCode::CONFLICT, "stale_version"),
            ServerError::NotReversible(_) => (StatusCode::CONFLICT, "not_reversible"),
            ServerError::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
            ServerError::UnsupportedMedia(_) => (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media"),
            ServerError::Busy => (StatusCode::TOO_MANY_REQUESTS, "busy"),
            ServerError::Unavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
            ServerError::NotCaughtUp { .. } => (StatusCode::SERVICE_UNAVAILABLE, "not_caught_up"),
            ServerError::AlreadyBootstrapped => (StatusCode::CONFLICT, "conflict"),
        };
        // Backend messages can carry internal detail, so callers get a plain one. The detail goes to the log.
        let message = if let ServerError::Unavailable(detail) = &e {
            tracing::error!(request_id, detail, "backend unavailable");
            "a backend is unavailable, so try again".to_string()
        } else {
            e.to_string()
        };
        let current = if let ServerError::StaleVersion { current, .. } = &e {
            serde_json::to_value(current).ok()
        } else {
            None
        };
        Self {
            current,
            ..Self::new(status, code, message, request_id)
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: ErrorDetail {
                    code: self.code.into(),
                    message: self.message,
                    request_id: self.request_id,
                    current: self.current,
                },
            }),
        )
            .into_response()
    }
}
