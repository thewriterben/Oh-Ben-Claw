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
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": message }))).into_response()
}

/// Operate-tier middleware (Ecosystem Integration I4).
///
/// Read requests (GET/HEAD) always pass through. Mutating requests:
/// 1. When `gateway.operate_token` is configured, require the
///    `X-OBC-Operate: <token>` header (constant-time compare) — read-only by
///    default, explicit elevation for remote actions. When unconfigured,
///    behavior is unchanged (local-console compatibility).
/// 2. Are appended to the Track 0 tamper-evident action audit (when wired),
///    so remote operation is MAC-chained and signed like any physical action.
pub async fn require_operate(
    State(state): State<Arc<GatewayState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let method = request.method().clone();
    if method == axum::http::Method::GET || method == axum::http::Method::HEAD {
        return next.run(request).await;
    }

    if let Some(ref expected) = state.config.operate_token {
        let provided = request
            .headers()
            .get("X-OBC-Operate")
            .and_then(|v| v.to_str().ok());
        match provided {
            Some(token) if constant_time_eq(token.as_bytes(), expected.as_bytes()) => {}
            Some(_) => return forbidden("Invalid operate token"),
            None => {
                return forbidden(
                    "Mutating requests require the X-OBC-Operate header (Operate mode)",
                )
            }
        }
    }

    // Signed remote-action audit: chain the request into the Track 0 log.
    // Best-effort — auditing must never block the action path (same policy as
    // the agent's own physical-action auditing).
    if let Some(audit) = &state.action_audit {
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let path = request.uri().path().to_string();
        let args = json!({ "method": method.as_str(), "path": path });
        if let Ok(mut auditor) = audit.lock() {
            let _ = auditor.record(
                ts_ms,
                "remote-operator",
                &format!("gateway:{} {}", method.as_str(), path),
                &args,
                crate::tools::traits::RiskClass::default(),
                crate::security::audit::Decision::Allowed,
            );
        }
    }

    next.run(request).await
}

fn forbidden(message: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({ "error": message }))).into_response()
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
