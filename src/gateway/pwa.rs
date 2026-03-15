//! Progressive Web App (PWA) embedded web client.
//!
//! The PWA is a single-page application served directly from the Oh-Ben-Claw
//! binary — no separate web server or build step required. It provides a
//! mobile-friendly chat interface that connects to the gateway via WebSocket.
//!
//! # Features
//!
//! - Dark-themed, mobile-first responsive design
//! - Real-time chat via WebSocket
//! - Tool call and result display with expandable details
//! - Peripheral node status panel
//! - Installable as a home screen app (PWA manifest + service worker)
//! - Works offline for viewing history (service worker caches assets)

use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};

// Embed the PWA assets at compile time using include_str!
// This avoids doc-test parser issues with HTML content in raw string literals.
const PWA_HTML: &str = include_str!("assets/index.html");
const PWA_MANIFEST: &str = include_str!("assets/manifest.json");
const SERVICE_WORKER_JS: &str = include_str!("assets/sw.js");

/// Serve the PWA index.html.
pub async fn serve_index() -> impl IntoResponse {
    Html(PWA_HTML)
}

/// Serve the PWA manifest.json for installability.
pub async fn serve_manifest() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/manifest+json")],
        PWA_MANIFEST,
    )
        .into_response()
}

/// Serve the service worker for offline support.
pub async fn serve_service_worker() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/javascript")],
        SERVICE_WORKER_JS,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pwa_html_is_valid_html() {
        assert!(PWA_HTML.contains("<!DOCTYPE html>"));
        assert!(PWA_HTML.contains("</html>"));
        assert!(PWA_HTML.contains("Oh-Ben-Claw"));
        assert!(PWA_HTML.contains("/ws"));
    }

    #[test]
    fn pwa_manifest_is_valid_json() {
        let v: serde_json::Value = serde_json::from_str(PWA_MANIFEST).unwrap();
        assert_eq!(v["name"], "Oh-Ben-Claw");
        assert_eq!(v["display"], "standalone");
    }

    #[test]
    fn service_worker_contains_cache_logic() {
        assert!(SERVICE_WORKER_JS.contains("CACHE_NAME"));
        assert!(SERVICE_WORKER_JS.contains("install"));
        assert!(SERVICE_WORKER_JS.contains("activate"));
        assert!(SERVICE_WORKER_JS.contains("fetch"));
    }

    #[tokio::test]
    async fn serve_index_returns_html() {
        let response = serve_index().await.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn serve_manifest_returns_json() {
        let response = serve_manifest().await;
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(content_type.contains("manifest+json"));
    }
}
