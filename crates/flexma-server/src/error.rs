use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub message: String,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> AppError {
        AppError { status, message: message.into() }
    }
    pub fn bad_request(m: impl Into<String>) -> AppError {
        AppError::new(StatusCode::BAD_REQUEST, m)
    }
    pub fn unauthorized() -> AppError {
        AppError::new(StatusCode::UNAUTHORIZED, "not signed in")
    }
    pub fn forbidden(m: impl Into<String>) -> AppError {
        AppError::new(StatusCode::FORBIDDEN, m)
    }
    pub fn not_found(what: &str) -> AppError {
        AppError::new(StatusCode::NOT_FOUND, format!("{what} not found"))
    }
    pub fn conflict(m: impl Into<String>) -> AppError {
        AppError::new(StatusCode::CONFLICT, m)
    }
    pub fn internal(m: impl std::fmt::Display) -> AppError {
        tracing::error!("internal error: {m}");
        AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::internal(e)
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::bad_request(e.to_string())
    }
}
