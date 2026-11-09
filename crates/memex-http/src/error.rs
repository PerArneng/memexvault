use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use memex_api::{ErrorBody, ErrorDetail};
use memex_core::ServerError;

pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    request_id: String,
    current: Option<serde_json::Value>,
    retry_after_s: Option<u64>,
}

impl ApiError {
    pub fn is_not_caught_up(&self) -> bool {
        self.code == "not_caught_up"
    }

    /// The two parts a batch reports for each decision that failed.
    pub fn code_and_message(self) -> (&'static str, String) {
        (self.code, self.message)
    }

    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>, request_id: &str) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            request_id: request_id.into(),
            current: None,
            retry_after_s: None,
        }
    }

    pub fn from_server(e: ServerError, request_id: &str) -> Self {
        let status = match &e {
            ServerError::Unauthenticated => StatusCode::UNAUTHORIZED,
            ServerError::Forbidden | ServerError::ChatDisabled => StatusCode::FORBIDDEN,
            ServerError::NotFound => StatusCode::NOT_FOUND,
            ServerError::BlobMissing => StatusCode::GONE,
            ServerError::Invalid(_) => StatusCode::BAD_REQUEST,
            ServerError::Conflict(_) | ServerError::StaleVersion { .. } | ServerError::NotReversible(_) | ServerError::AlreadyBootstrapped => {
                StatusCode::CONFLICT
            }
            ServerError::TooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            ServerError::UnsupportedMedia(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ServerError::Busy | ServerError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            ServerError::Unavailable(_) | ServerError::NotCaughtUp { .. } => StatusCode::SERVICE_UNAVAILABLE,
        };
        let (code, message) = (memex_api::error_code(&e), memex_api::error_message(&e, request_id));
        let current = if let ServerError::StaleVersion { current, .. } = &e {
            serde_json::to_value(current).ok()
        } else {
            None
        };
        let retry_after_s = if let ServerError::RateLimited { retry_after_s } = &e {
            Some(*retry_after_s)
        } else {
            None
        };
        Self {
            current,
            retry_after_s,
            ..Self::new(status, code, message, request_id)
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let retry_after = self.retry_after_s.map(|s| [(header::RETRY_AFTER, s.to_string())]);
        (
            self.status,
            retry_after,
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
