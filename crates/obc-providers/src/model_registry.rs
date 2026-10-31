//! Per-node, local-first model selection with health re-checks.
//!
//! An edge node should prefer a **local** model (on-device Ollama, no network, no
//! cost) and fall back to a cloud model only when the local one is unavailable.
//! This registry orders candidates local-first by priority and selects the highest
//! one that is healthy — *or whose health check has gone stale*. That last clause
//! is the important fix over the pattern this is adapted from, where a model marked
//! unavailable was cached as down **forever** and never retried: here an unhealthy
//! model becomes eligible again after `recheck_after_ms`, so a node that briefly
//! lost its local model recovers to it automatically instead of staying on cloud.
//!
//! Plan P3 gave it callers: `start` builds one for the edge node's local models,
//! [`probe`]s each, and attaches world memory, so every change of health becomes
//! an `edge.model.<name>` fact the world-state block shows as up or down. The
//! router's local back-off and the reflex tier's calls feed it from then on.

use crate::ProviderConfig;
use obc_memory::world::{Origin, WorldMemory};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One selectable model endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    /// Model id (e.g. `"llama3.2"`, `"gpt-4o"`).
    pub name: String,
    /// How to reach it (endpoint URL or provider key).
    pub endpoint: String,
    /// Whether it runs locally on the node (preferred).
    pub local: bool,
    /// Tie-break within local/remote (lower = preferred).
    pub priority: u32,
}

#[derive(Debug, Clone, Copy)]
struct Health {
    healthy: bool,
    checked_at_ms: u64,
}

/// A node's model registry: ordered candidates + live health, local-first.
pub struct ModelRegistry {
    models: Vec<ModelEntry>,
    health: Mutex<HashMap<String, Health>>,
    /// How long an unhealthy mark lasts before the model is retried.
    recheck_after_ms: u64,
    /// Where each change of health is written as `edge.model.<name>`.
    world: Option<Arc<WorldMemory>>,
}

impl ModelRegistry {
    /// Build a registry. Candidates are ordered **local-first**, then by ascending
    /// `priority`. `recheck_after_ms` is how long a failed model stays out before
    /// it becomes eligible to retry.
    pub fn new(models: impl IntoIterator<Item = ModelEntry>, recheck_after_ms: u64) -> Self {
        let mut models: Vec<ModelEntry> = models.into_iter().collect();
        models.sort_by(|a, b| {
            // local before remote, then lower priority first
            b.local.cmp(&a.local).then(a.priority.cmp(&b.priority))
        });
        Self {
            models,
            health: Mutex::new(HashMap::new()),
            recheck_after_ms,
            world: None,
        }
    }

    /// Write each change of a model's health to `world` as the fact
    /// `edge.model.<name>` = `{"status": "up"|"down", "local": bool}`.
    pub fn with_world(mut self, world: Arc<WorldMemory>) -> Self {
        self.world = Some(world);
        self
    }

    /// Record the result of a health probe against `model`. With world memory
    /// attached, a first result or a change of state is written as a fact; a
    /// repeat of the same state only refreshes the timestamp here.
    pub fn record_health(&self, model: &str, healthy: bool, now_ms: u64) {
        let previous = self.health.lock().unwrap().insert(
            model.to_string(),
            Health {
                healthy,
                checked_at_ms: now_ms,
            },
        );
        if previous.is_some_and(|h| h.healthy == healthy) {
            return;
        }
        let Some(world) = &self.world else { return };
        let local = self
            .models
            .iter()
            .find(|m| m.name == model)
            .is_some_and(|m| m.local);
        let value = serde_json::json!({
            "status": if healthy { "up" } else { "down" },
            "local": local,
        });
        if let Err(e) = world.observe_as(
            &format!("edge.model.{model}"),
            value,
            now_ms,
            now_ms,
            "model_registry",
            Origin::Derived,
        ) {
            tracing::debug!(error = %e, model, "model registry: could not record health");
        }
    }

    /// Whether `model` may be called right now: never probed, healthy, or
    /// unhealthy long enough ago to be worth retrying.
    pub fn usable(&self, model: &str, now_ms: u64) -> bool {
        match self.health.lock().unwrap().get(model) {
            None => true,
            Some(h) => h.healthy || now_ms.saturating_sub(h.checked_at_ms) >= self.recheck_after_ms,
        }
    }

    /// Select the preferred model usable right now: the highest-ordered candidate
    /// that is healthy, never-probed, or whose unhealthy mark has gone stale.
    pub fn select(&self, now_ms: u64) -> Option<&ModelEntry> {
        let health = self.health.lock().unwrap();
        let idx = self.models.iter().position(|m| match health.get(&m.name) {
            None => true,                 // never probed → optimistically try
            Some(h) if h.healthy => true, // known good
            Some(h) => now_ms.saturating_sub(h.checked_at_ms) >= self.recheck_after_ms, // stale → retry
        });
        drop(health);
        idx.map(|i| &self.models[i])
    }

    /// The candidate ordering (local-first), for inspection.
    pub fn candidates(&self) -> &[ModelEntry] {
        &self.models
    }
}

// ── Bridge to the provider config (edge local-first selection) ──────────────────

impl ModelEntry {
    /// Derive a registry entry from a provider config. Treated as **local** when the
    /// provider is Ollama or its base URL is a loopback address.
    pub fn from_provider(cfg: &ProviderConfig, priority: u32) -> Self {
        let local = cfg.name.eq_ignore_ascii_case("ollama")
            || cfg.base_url.as_deref().map(is_loopback).unwrap_or(false);
        Self {
            name: cfg.model.clone(),
            endpoint: cfg.base_url.clone().unwrap_or_default(),
            local,
            priority,
        }
    }
}

fn is_loopback(url: &str) -> bool {
    url.contains("localhost")
        || url.contains("127.0.0.1")
        || url.contains("[::1]")
        || url.contains("0.0.0.0")
}

/// Flatten a primary provider config + its fallback chain into a candidate list
/// `[primary, fallback0, fallback1, …]` (each without its own nested fallbacks).
pub fn flatten_candidates(primary: &ProviderConfig) -> Vec<ProviderConfig> {
    let mut head = primary.clone();
    let fallbacks = std::mem::take(&mut head.fallbacks);
    let mut out = Vec::with_capacity(1 + fallbacks.len());
    out.push(head);
    out.extend(fallbacks);
    out
}

/// Build a [`ModelRegistry`] from an ordered candidate list (index = priority).
pub fn registry_from_providers(configs: &[ProviderConfig], recheck_after_ms: u64) -> ModelRegistry {
    ModelRegistry::new(
        configs
            .iter()
            .enumerate()
            .map(|(i, c)| ModelEntry::from_provider(c, i as u32)),
        recheck_after_ms,
    )
}

/// Probe whether a provider's server answers, within `timeout`.
///
/// Ollama (`base_url` ending `/api/chat`, or none) is asked for `/api/tags`
/// and must list the model, with or without its `:latest` tag. Anything else
/// is taken to be OpenAI-compatible, as llama.cpp's server is: its
/// `/chat/completions` URL becomes `/models`, and any success is up. A refused
/// connection, an error status or the timeout is down.
pub async fn probe(cfg: &ProviderConfig, timeout: Duration) -> bool {
    let Ok(client) = reqwest::Client::builder().timeout(timeout).build() else {
        return false;
    };
    let ollama = cfg.name.eq_ignore_ascii_case("ollama");
    let url = if ollama {
        let base = cfg
            .base_url
            .as_deref()
            .unwrap_or("http://localhost:11434/api/chat");
        format!("{}/api/tags", base.trim_end_matches("/api/chat"))
    } else {
        let Some(base) = cfg.base_url.as_deref() else {
            return false;
        };
        format!("{}/models", base.trim_end_matches("/chat/completions"))
    };
    let resp = match client.get(&url).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return false,
    };
    if !ollama {
        return true;
    }
    let Ok(tags) = resp.json::<serde_json::Value>().await else {
        return false;
    };
    let want = cfg.model.trim_end_matches(":latest");
    tags["models"].as_array().is_some_and(|ms| {
        ms.iter().any(|m| {
            m["name"]
                .as_str()
                .is_some_and(|n| n.trim_end_matches(":latest") == want)
        })
    })
}

/// Select the preferred usable provider config — local-first, health-aware — from a
/// candidate list and its registry. `None` if every candidate is freshly unhealthy.
pub fn select_provider<'a>(
    configs: &'a [ProviderConfig],
    registry: &ModelRegistry,
    now_ms: u64,
) -> Option<&'a ProviderConfig> {
    let entry = registry.select(now_ms)?;
    configs.iter().find(|c| c.model == entry.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, local: bool, priority: u32) -> ModelEntry {
        ModelEntry {
            name: name.into(),
            endpoint: format!("http://{name}"),
            local,
            priority,
        }
    }

    fn registry() -> ModelRegistry {
        ModelRegistry::new(
            [
                entry("gpt-4o", false, 0),
                entry("llama3.2", true, 0),
                entry("claude", false, 1),
            ],
            10_000,
        )
    }

    #[test]
    fn local_model_is_preferred() {
        let r = registry();
        assert_eq!(r.candidates()[0].name, "llama3.2", "local first");
        assert_eq!(r.select(1_000).unwrap().name, "llama3.2");
    }

    #[test]
    fn falls_back_when_the_local_model_is_unhealthy() {
        let r = registry();
        r.record_health("llama3.2", false, 1_000);
        // local is down → first healthy remote
        assert_eq!(r.select(1_000).unwrap().name, "gpt-4o");
    }

    #[test]
    fn a_stale_unhealthy_mark_is_retried_not_cached_forever() {
        let r = registry();
        r.record_health("llama3.2", false, 1_000); // marked down at t=1s
        assert_eq!(
            r.select(2_000).unwrap().name,
            "gpt-4o",
            "still on fallback while fresh"
        );
        // after the recheck window the local model is eligible again
        assert_eq!(
            r.select(12_000).unwrap().name,
            "llama3.2",
            "stale mark → retry local"
        );
    }

    #[test]
    fn all_unhealthy_and_fresh_selects_nothing() {
        let r = registry();
        for m in ["llama3.2", "gpt-4o", "claude"] {
            r.record_health(m, false, 1_000);
        }
        assert!(
            r.select(2_000).is_none(),
            "no usable model while all are freshly down"
        );
        // but once stale, selection resumes (local first)
        assert_eq!(r.select(12_000).unwrap().name, "llama3.2");
    }

    fn provider(name: &str, model: &str, base_url: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            name: name.into(),
            model: model.into(),
            api_key: None,
            base_url: base_url.map(String::from),
            temperature: 0.7,
            fallbacks: vec![],
            retry: None,
            response_format: None,
            prompt_caching: true,
            think: None,
            routing: None,
        }
    }

    #[test]
    fn ollama_and_loopback_are_local() {
        assert!(
            ModelEntry::from_provider(
                &provider("ollama", "llama3.2", Some("http://localhost:11434")),
                0
            )
            .local
        );
        assert!(
            ModelEntry::from_provider(&provider("compat", "m", Some("http://127.0.0.1:8080")), 0)
                .local
        );
        assert!(!ModelEntry::from_provider(&provider("openai", "gpt-4o", None), 0).local);
    }

    #[test]
    fn flatten_pulls_primary_and_fallbacks() {
        let mut primary = provider("openai", "gpt-4o", None);
        primary.fallbacks = vec![provider(
            "ollama",
            "llama3.2",
            Some("http://localhost:11434"),
        )];
        let flat = flatten_candidates(&primary);
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].model, "gpt-4o");
        assert!(
            flat[0].fallbacks.is_empty(),
            "the primary keeps no nested fallbacks"
        );
        assert_eq!(flat[1].model, "llama3.2");
    }

    #[test]
    fn selection_prefers_the_local_fallback_over_a_cloud_primary() {
        let mut primary = provider("openai", "gpt-4o", None);
        primary.fallbacks = vec![provider(
            "ollama",
            "llama3.2",
            Some("http://localhost:11434"),
        )];
        let candidates = flatten_candidates(&primary);
        let registry = registry_from_providers(&candidates, 60_000);
        let chosen = select_provider(&candidates, &registry, 0).unwrap();
        assert_eq!(chosen.name, "ollama", "edge prefers the on-device model");
    }

    #[test]
    fn selection_falls_back_to_cloud_when_local_is_down() {
        let mut primary = provider("openai", "gpt-4o", None);
        primary.fallbacks = vec![provider(
            "ollama",
            "llama3.2",
            Some("http://localhost:11434"),
        )];
        let candidates = flatten_candidates(&primary);
        let registry = registry_from_providers(&candidates, 60_000);
        registry.record_health("llama3.2", false, 1_000);
        let chosen = select_provider(&candidates, &registry, 1_000).unwrap();
        assert_eq!(chosen.name, "openai", "local down → cloud fallback");
    }

    #[test]
    fn health_changes_become_edge_model_facts() {
        let world = Arc::new(WorldMemory::open_in_memory().unwrap());
        let r = registry().with_world(Arc::clone(&world));
        r.record_health("llama3.2", true, 1_000);
        let up = world.current("edge.model.llama3.2").unwrap().unwrap();
        assert_eq!(up.value, serde_json::json!({"status": "up", "local": true}));
        r.record_health("llama3.2", true, 2_000);
        assert_eq!(
            world.history("edge.model.llama3.2").unwrap().len(),
            1,
            "a repeat of the same state writes nothing"
        );
        r.record_health("llama3.2", false, 3_000);
        let down = world.current("edge.model.llama3.2").unwrap().unwrap();
        assert_eq!(down.value["status"], "down");
        assert!(!r.usable("llama3.2", 3_500));
        assert!(r.usable("llama3.2", 13_000), "stale: worth a retry");
        assert!(r.usable("never-probed", 0));
    }

    /// Answer every connection on a loopback port with `status` and `body`.
    fn serve(status: &'static str, body: &'static str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut conn in listener.incoming().flatten() {
                let mut buf = [0u8; 2048];
                let _ = conn.read(&mut buf);
                let _ = write!(
                    conn,
                    "HTTP/1.1 {status}
Content-Type: application/json
Content-Length: {}
Connection: close

{body}",
                    body.len()
                );
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    #[tokio::test]
    async fn probe_asks_ollama_for_the_model_and_llama_cpp_for_any_answer() {
        let t = Duration::from_secs(2);
        let tags = serve("200 OK", r#"{"models":[{"name":"gemma3n:latest"}]}"#);
        let mut ollama = provider("ollama", "gemma3n", Some(&format!("{tags}/api/chat")));
        assert!(probe(&ollama, t).await);
        ollama.model = "llama3.2".into();
        assert!(
            !probe(&ollama, t).await,
            "the server is up; the model is not"
        );

        let models = serve("200 OK", r#"{"data":[]}"#);
        let cpp = provider(
            "compatible",
            "gemma-3n",
            Some(&format!("{models}/v1/chat/completions")),
        );
        assert!(probe(&cpp, t).await);
        let broken = serve("500 Internal Server Error", "{}");
        let cpp = provider(
            "compatible",
            "gemma-3n",
            Some(&format!("{broken}/v1/chat/completions")),
        );
        assert!(!probe(&cpp, t).await);

        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        let gone = provider(
            "compatible",
            "gemma-3n",
            Some(&format!("http://127.0.0.1:{port}/v1/chat/completions")),
        );
        assert!(!probe(&gone, t).await, "nothing listening is down");
    }
}
