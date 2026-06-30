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

use std::collections::HashMap;
use std::sync::Mutex;

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
        Self { models, health: Mutex::new(HashMap::new()), recheck_after_ms }
    }

    /// Record the result of a health probe against `model`.
    pub fn record_health(&self, model: &str, healthy: bool, now_ms: u64) {
        self.health
            .lock()
            .unwrap()
            .insert(model.to_string(), Health { healthy, checked_at_ms: now_ms });
    }

    /// Select the preferred model usable right now: the highest-ordered candidate
    /// that is healthy, never-probed, or whose unhealthy mark has gone stale.
    pub fn select(&self, now_ms: u64) -> Option<&ModelEntry> {
        let health = self.health.lock().unwrap();
        let idx = self.models.iter().position(|m| match health.get(&m.name) {
            None => true,                       // never probed → optimistically try
            Some(h) if h.healthy => true,       // known good
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, local: bool, priority: u32) -> ModelEntry {
        ModelEntry { name: name.into(), endpoint: format!("http://{name}"), local, priority }
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
        assert_eq!(r.select(2_000).unwrap().name, "gpt-4o", "still on fallback while fresh");
        // after the recheck window the local model is eligible again
        assert_eq!(r.select(12_000).unwrap().name, "llama3.2", "stale mark → retry local");
    }

    #[test]
    fn all_unhealthy_and_fresh_selects_nothing() {
        let r = registry();
        for m in ["llama3.2", "gpt-4o", "claude"] {
            r.record_health(m, false, 1_000);
        }
        assert!(r.select(2_000).is_none(), "no usable model while all are freshly down");
        // but once stale, selection resumes (local first)
        assert_eq!(r.select(12_000).unwrap().name, "llama3.2");
    }
}
