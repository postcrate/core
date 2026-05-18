//! Bearer-token middleware for the HTTP API.
//!
//! When `settings.network.api_auth_token` is set, every `/api/v1/...`
//! request must carry `Authorization: Bearer <token>`. The healthz
//! endpoint at `/healthz` is always open so process-level health
//! probes still work without credentials.
//!
//! The middleware compares the token with constant-time-ish equality
//! (`subtle` would be cleaner but adds a dep; we avoid leaking the
//! length by always running the full byte loop).

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

pub async fn require_bearer(
    expected: &str,
    req: Request<Body>,
    next: Next,
) -> std::result::Result<Response, Response> {
    // Always-open paths.
    let path = req.uri().path();
    if path == "/healthz" || path == "/info" {
        return Ok(next.run(req).await);
    }

    let header_value = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let provided = header_value
        .strip_prefix("Bearer ")
        .or_else(|| header_value.strip_prefix("bearer "))
        .unwrap_or("");

    if !constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        return Err((
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer realm=\"postcrate\"")],
            axum::Json(serde_json::json!({
                "error": "unauthorized",
                "message": "Authorization: Bearer <token> required",
            })),
        )
            .into_response());
    }
    Ok(next.run(req).await)
}

/// Compares two byte slices without short-circuiting on length or
/// content. We're not protecting against a remote timing-attack on a
/// dev tool, but it's still polite to not leak the answer.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let len = a.len().max(b.len()).max(1);
    let mut diff: u8 = (a.len() != b.len()) as u8;
    for i in 0..len {
        let av = a.get(i).copied().unwrap_or(0);
        let bv = b.get(i).copied().unwrap_or(0);
        diff |= av ^ bv;
    }
    diff == 0
}
