//! `IntoResponse` for our crate `Error`. The wire shape is the
//! `{error, message}` object used elsewhere in the API.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::error::Error;

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
    message: String,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        // Log internal-class errors at warn level so they show up in
        // CI logs without spamming production output for 4xx cases.
        if self.http_status().as_u16() >= 500 {
            tracing::warn!(target: "postcrate::http", error = %self, code = self.code());
        }
        let status = self.http_status();
        let body = ErrorBody {
            error: self.code(),
            message: self.to_string(),
        };
        // We can't safely map every error variant to a non-500 — if a
        // DB error somehow leaks, give the client a 500 not a panic.
        let status = if matches!(status, StatusCode::OK) {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            status
        };
        (status, Json(body)).into_response()
    }
}
