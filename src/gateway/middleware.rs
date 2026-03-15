//! Gateway middleware — authentication and request validation.

use super::GatewayState;
use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::sync::Arc;

/// Bearer token authentication middleware.
///
/// If `gateway.api_token` is set in config, this middleware validates the
/// `Authorization: Bearer <token>` header on every API request.
pub async fn require_auth(
    State(state): State<Arc<GatewayState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let Some(ref expected_token) = state.config.api_token else {
        // No token configured — allow all requests
        return next.run(request).await;
    };

    let auth_header = request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header["Bearer ".len()..];
            if constant_time_eq(token.as_bytes(), expected_token.as_bytes()) {
                next.run(request).await
            } else {
                unauthorized("Invalid token")
            }
        }
        Some(_) => unauthorized("Authorization header must use Bearer scheme"),
        None => unauthorized("Authorization header required"),
    }
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": message })),
    )
        .into_response()
}

/// Constant-time byte comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_same() {
        assert!(constant_time_eq(b"hello", b"hello"));
    }

    #[test]
    fn constant_time_eq_different() {
        assert!(!constant_time_eq(b"hello", b"world"));
    }

    #[test]
    fn constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"hello", b"hello!"));
    }

    #[test]
    fn constant_time_eq_empty() {
        assert!(constant_time_eq(b"", b""));
    }
}
