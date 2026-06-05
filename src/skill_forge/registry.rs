//! ClawHub Skill Registry — community-driven skill marketplace.
//!
//! ClawHub is the public skill registry introduced alongside OpenClaw 3.13
//! (March 2026).  It provides a curated catalogue of pre-built automation
//! skills ("skills") that any Oh-Ben-Claw user can search, install, and manage
//! from the command line or the REST API.
//!
//! # Architecture
//!
//! ```text
//! ClawHubClient  ──HTTP──▶  ClawHub REST API  ──▶  JSON index
//!       │
//!       ├── search(query)  →  Vec<ClawHubEntry>
//!       ├── fetch_manifest(name, version) → SkillManifest
//!       └── install(entry, dir)  →  writes .skill.json to skills dir
//! ```
//!
//! # Usage
//!
//! ```rust,no_run
//! use oh_ben_claw::skill_forge::registry::ClawHubClient;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let client = ClawHubClient::new("https://hub.openclaw.ai");
//! let results = client.search("weather").await?;
//! for entry in &results {
//!     println!("{} v{} — {}", entry.name, entry.version, entry.description);
//! }
//! # Ok(())
//! # }
//! ```

use crate::skill_forge::install_policy::{
    iso8601_now, sha256_hex, InstallAuditEntry, InstallAuditLog, InstallConsent, InstallDecision,
    InstallInspection, InstallPolicy, InstallPolicyConfig,
};
use crate::skill_forge::SkillManifest;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

// ── ClawHub entry ─────────────────────────────────────────────────────────────

/// A single entry in the ClawHub skill catalogue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClawHubEntry {
    /// Unique skill name (snake_case).
    pub name: String,
    /// Published version string (SemVer).
    pub version: String,
    /// Short description of what the skill does.
    pub description: String,
    /// Author's GitHub handle or display name.
    pub author: String,
    /// Number of times this skill has been installed.
    #[serde(default)]
    pub downloads: u64,
    /// Star rating (0–5).
    #[serde(default)]
    pub stars: f32,
    /// Free-form tags for search filtering.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Whether this entry has been verified by the ClawHub team.
    #[serde(default)]
    pub verified: bool,
    /// Direct URL to the `.skill.json` manifest.
    pub manifest_url: String,
    /// SHA-256 of the manifest bytes, when the registry publishes one
    /// (mandatory for new ClawHub submissions since the 2026 signing rollout).
    #[serde(default)]
    pub sha256: Option<String>,
}

impl ClawHubEntry {
    /// Returns `true` if the entry is marked as verified by ClawHub.
    pub fn is_verified(&self) -> bool {
        self.verified
    }

    /// Returns `true` if any tag matches `tag` (case-insensitive).
    pub fn has_tag(&self, tag: &str) -> bool {
        let lower = tag.to_lowercase();
        self.tags.iter().any(|t| t.to_lowercase() == lower)
    }
}

// ── Local skill index ─────────────────────────────────────────────────────────

/// A locally-cached index of skills that have been searched or installed.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SkillRegistryIndex {
    /// All entries loaded from the remote catalogue or cache.
    pub entries: Vec<ClawHubEntry>,
    /// Timestamp of the last successful index refresh (Unix seconds).
    pub refreshed_at: Option<u64>,
    /// Base URL of the registry the index was fetched from.
    pub registry_url: String,
}

impl SkillRegistryIndex {
    /// Create a new empty index for the given registry URL.
    pub fn new(registry_url: impl Into<String>) -> Self {
        Self {
            entries: Vec::new(),
            refreshed_at: None,
            registry_url: registry_url.into(),
        }
    }

    /// Search the index for entries matching `query` in name, description, or tags.
    pub fn search(&self, query: &str) -> Vec<&ClawHubEntry> {
        let q = query.to_lowercase();
        self.entries
            .iter()
            .filter(|e| {
                e.name.to_lowercase().contains(&q)
                    || e.description.to_lowercase().contains(&q)
                    || e.tags.iter().any(|t| t.to_lowercase().contains(&q))
            })
            .collect()
    }

    /// Return the entry with the given name, if present.
    pub fn find(&self, name: &str) -> Option<&ClawHubEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Total number of entries in this index.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the index contains no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ── ClawHubClient ─────────────────────────────────────────────────────────────

/// HTTP client for interacting with a ClawHub registry API.
///
/// The client speaks a simple REST API:
///
/// ```text
/// GET  /api/v1/skills          → JSON array of ClawHubEntry
/// GET  /api/v1/skills/{name}   → single ClawHubEntry
/// GET  /api/v1/skills/{name}/{version}/manifest → SkillManifest
/// ```
#[derive(Debug, Clone)]
pub struct ClawHubClient {
    /// Base URL of the ClawHub registry (e.g. `https://hub.openclaw.ai`).
    pub base_url: String,
    client: reqwest::Client,
    /// Locally cached index.
    index: std::sync::Arc<tokio::sync::RwLock<SkillRegistryIndex>>,
    /// Install-security policy (Phase 15, WS1).
    policy: InstallPolicy,
    /// Append-only JSONL audit log for install decisions.
    audit_log: InstallAuditLog,
}

impl ClawHubClient {
    /// Create a client pointing at `base_url` with the default (secure)
    /// install policy: every install requires explicit operator consent.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::with_policy(base_url, InstallPolicyConfig::default())
    }

    /// Create a client with an explicit install-policy configuration.
    pub fn with_policy(base_url: impl Into<String>, policy_config: InstallPolicyConfig) -> Self {
        let base_url = base_url.into();
        let index = SkillRegistryIndex::new(&base_url);
        let audit_path = policy_config
            .audit_log_path
            .clone()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(InstallAuditLog::default_path);
        Self {
            base_url,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            index: std::sync::Arc::new(tokio::sync::RwLock::new(index)),
            policy: InstallPolicy::new(policy_config),
            audit_log: InstallAuditLog::new(audit_path),
        }
    }

    /// The active install policy.
    pub fn policy(&self) -> &InstallPolicy {
        &self.policy
    }

    /// The install audit log.
    pub fn audit_log(&self) -> &InstallAuditLog {
        &self.audit_log
    }

    /// Host portion of the registry base URL (for external-URL flagging).
    fn registry_host(&self) -> String {
        self.base_url
            .strip_prefix("https://")
            .or_else(|| self.base_url.strip_prefix("http://"))
            .unwrap_or(&self.base_url)
            .split(['/', ':'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// Search the remote registry for skills matching `query`.
    ///
    /// Results are cached in the local index for offline use.
    pub async fn search(&self, query: &str) -> Result<Vec<ClawHubEntry>> {
        // Try to use the cached index first.
        {
            let idx = self.index.read().await;
            if !idx.is_empty() {
                return Ok(idx.search(query).into_iter().cloned().collect());
            }
        }

        // Fetch the full catalogue from the remote.
        self.refresh_index().await?;
        let idx = self.index.read().await;
        Ok(idx.search(query).into_iter().cloned().collect())
    }

    /// Fetch a specific skill entry by name.
    pub async fn get_entry(&self, name: &str) -> Result<Option<ClawHubEntry>> {
        let url = format!("{}/api/v1/skills/{}", self.base_url, name);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let entry: ClawHubEntry = response
            .json()
            .await
            .with_context(|| format!("Deserialize ClawHubEntry from {url}"))?;
        Ok(Some(entry))
    }

    /// Fetch the full `SkillManifest` for a specific skill version.
    pub async fn fetch_manifest(&self, name: &str, version: &str) -> Result<SkillManifest> {
        let url = format!(
            "{}/api/v1/skills/{}/{}/manifest",
            self.base_url, name, version
        );
        let manifest: SkillManifest = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?
            .json()
            .await
            .with_context(|| format!("Deserialize SkillManifest from {url}"))?;
        Ok(manifest)
    }

    /// Install a skill by downloading its manifest and writing it to `skills_dir`.
    ///
    /// **Security (Phase 15, WS1):** the install is gated by the client's
    /// [`InstallPolicy`] — allowlist, version pins, and checksum verification
    /// are enforced, the manifest is statically inspected (external URLs,
    /// shell execution, download-instruction language), and unless the policy
    /// disables it, explicit operator `consent` is required. Every decision —
    /// allowed or refused — is appended to the JSONL audit log.
    ///
    /// Call with [`InstallConsent::None`] first; if the result is an
    /// `ApprovalRequired` error, show the flags to the operator and retry with
    /// [`InstallConsent::Approved`].
    ///
    /// Returns the path of the written `.skill.json` file.
    pub async fn install(
        &self,
        entry: &ClawHubEntry,
        skills_dir: &Path,
        consent: InstallConsent,
    ) -> Result<std::path::PathBuf> {
        // Download the manifest as raw bytes (checksums apply to exact bytes).
        let manifest_bytes = self
            .client
            .get(&entry.manifest_url)
            .send()
            .await
            .with_context(|| format!("GET {}", entry.manifest_url))?
            .bytes()
            .await
            .with_context(|| "Read manifest body")?;

        let manifest_json: serde_json::Value = serde_json::from_slice(&manifest_bytes)
            .with_context(|| "Deserialize manifest JSON")?;

        // Validate it can be parsed as a SkillManifest.
        let _: SkillManifest = serde_json::from_value(manifest_json.clone())
            .with_context(|| format!("Invalid SkillManifest for '{}'", entry.name))?;

        // Static inspection + policy evaluation.
        let inspection =
            InstallInspection::inspect(&manifest_json, &self.registry_host(), entry.verified);
        let decision = self.policy.evaluate(
            &entry.name,
            &entry.version,
            &manifest_bytes,
            entry.sha256.as_deref(),
            &inspection,
            consent,
        );

        // Audit every decision, allowed or not.
        let audit = InstallAuditEntry {
            timestamp: iso8601_now(),
            skill: entry.name.clone(),
            version: entry.version.clone(),
            manifest_sha256: sha256_hex(&manifest_bytes),
            decision: decision.clone(),
            flags: inspection.flags.clone(),
            registry_url: self.base_url.clone(),
        };
        if let Err(e) = self.audit_log.record(&audit) {
            tracing::warn!(error = %e, "Failed to write skill install audit entry");
        }

        match decision {
            InstallDecision::Deny { reason } => {
                tracing::warn!(skill = %entry.name, %reason, "Skill install denied by policy");
                bail!("Install of '{}' denied: {reason}", entry.name);
            }
            InstallDecision::ApprovalRequired { flags } => {
                let flag_list = if flags.is_empty() {
                    "no flags raised".to_string()
                } else {
                    flags.join("; ")
                };
                bail!(
                    "Install of '{}' v{} requires operator approval. Inspection: {flag_list}. \
                     Retry with InstallConsent::Approved after review.",
                    entry.name,
                    entry.version
                );
            }
            InstallDecision::Allow => {}
        }

        // Write to disk.
        std::fs::create_dir_all(skills_dir)
            .with_context(|| format!("Create skills dir {:?}", skills_dir))?;

        let file_name = format!("{}.skill.json", entry.name);
        let dest = skills_dir.join(&file_name);

        let content = serde_json::to_string_pretty(&manifest_json)?;
        std::fs::write(&dest, content)
            .with_context(|| format!("Write skill manifest to {:?}", dest))?;

        tracing::info!(skill = %entry.name, version = %entry.version, path = ?dest, "Skill installed from ClawHub");
        Ok(dest)
    }

    /// Refresh the local index by fetching the full catalogue from the remote.
    pub async fn refresh_index(&self) -> Result<()> {
        let url = format!("{}/api/v1/skills", self.base_url);
        let entries: Vec<ClawHubEntry> = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?
            .json()
            .await
            .with_context(|| "Deserialize skill catalogue")?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut idx = self.index.write().await;
        idx.entries = entries;
        idx.refreshed_at = Some(now);
        tracing::info!(count = idx.entries.len(), "ClawHub index refreshed");
        Ok(())
    }

    /// Return a snapshot of the current local index (without fetching from remote).
    pub async fn local_index(&self) -> SkillRegistryIndex {
        self.index.read().await.clone()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(name: &str, tags: &[&str], verified: bool) -> ClawHubEntry {
        ClawHubEntry {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            description: format!("A skill that does {name}"),
            author: "test_author".to_string(),
            downloads: 42,
            stars: 4.5,
            tags: tags.iter().map(|&t| t.to_string()).collect(),
            verified,
            manifest_url: format!("https://hub.example.com/skills/{name}/1.0.0/manifest"),
            sha256: None,
        }
    }

    // ── ClawHubEntry ──────────────────────────────────────────────────────────

    #[test]
    fn entry_is_verified() {
        let e = make_entry("weather", &["internet", "weather"], true);
        assert!(e.is_verified());
    }

    #[test]
    fn entry_not_verified() {
        let e = make_entry("joke", &["fun"], false);
        assert!(!e.is_verified());
    }

    #[test]
    fn entry_has_tag_case_insensitive() {
        let e = make_entry("news", &["RSS", "internet"], true);
        assert!(e.has_tag("rss"));
        assert!(e.has_tag("RSS"));
        assert!(e.has_tag("Internet"));
        assert!(!e.has_tag("email"));
    }

    #[test]
    fn entry_serializes_and_deserializes() {
        let e = make_entry("translate", &["language"], true);
        let json = serde_json::to_string(&e).unwrap();
        let e2: ClawHubEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(e, e2);
    }

    // ── SkillRegistryIndex ────────────────────────────────────────────────────

    #[test]
    fn index_starts_empty() {
        let idx = SkillRegistryIndex::new("https://hub.example.com");
        assert!(idx.is_empty());
        assert_eq!(idx.len(), 0);
        assert!(idx.refreshed_at.is_none());
    }

    #[test]
    fn index_search_by_name() {
        let mut idx = SkillRegistryIndex::new("https://hub.example.com");
        idx.entries.push(make_entry("check_weather", &[], true));
        idx.entries.push(make_entry("send_email", &[], false));

        let results = idx.search("weather");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "check_weather");
    }

    #[test]
    fn index_search_by_description() {
        let mut idx = SkillRegistryIndex::new("https://hub.example.com");
        idx.entries.push(make_entry("news_feed", &[], true));
        // "news_feed" description is "A skill that does news_feed"
        let results = idx.search("does news_feed");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn index_search_by_tag() {
        let mut idx = SkillRegistryIndex::new("https://hub.example.com");
        idx.entries
            .push(make_entry("translate", &["language", "api"], true));
        idx.entries.push(make_entry("joke", &["fun"], false));

        let results = idx.search("language");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "translate");
    }

    #[test]
    fn index_search_no_match_returns_empty() {
        let mut idx = SkillRegistryIndex::new("https://hub.example.com");
        idx.entries.push(make_entry("translate", &[], true));
        let results = idx.search("nonexistent_xyz");
        assert!(results.is_empty());
    }

    #[test]
    fn index_find_by_name() {
        let mut idx = SkillRegistryIndex::new("https://hub.example.com");
        idx.entries.push(make_entry("gpio_blink", &[], true));
        idx.entries.push(make_entry("sensor_read", &[], false));

        assert!(idx.find("gpio_blink").is_some());
        assert!(idx.find("sensor_read").is_some());
        assert!(idx.find("missing").is_none());
    }

    #[test]
    fn index_len_reflects_entries() {
        let mut idx = SkillRegistryIndex::new("https://hub.example.com");
        idx.entries.push(make_entry("a", &[], true));
        idx.entries.push(make_entry("b", &[], true));
        idx.entries.push(make_entry("c", &[], false));
        assert_eq!(idx.len(), 3);
        assert!(!idx.is_empty());
    }

    // ── ClawHubClient ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn client_local_index_starts_empty() {
        let client = ClawHubClient::new("https://hub.example.com");
        let idx = client.local_index().await;
        assert!(idx.is_empty());
        assert_eq!(idx.registry_url, "https://hub.example.com");
    }

    #[tokio::test]
    async fn client_search_with_populated_local_index() {
        let client = ClawHubClient::new("https://hub.example.com");
        // Populate the index directly (bypassing HTTP).
        {
            let mut idx = client.index.write().await;
            idx.entries
                .push(make_entry("weather_check", &["weather"], true));
            idx.entries.push(make_entry("joke_gen", &["fun"], false));
        }

        let results = client.search("weather").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "weather_check");
    }

    #[tokio::test]
    async fn client_search_returns_empty_for_no_match() {
        let client = ClawHubClient::new("https://hub.example.com");
        {
            let mut idx = client.index.write().await;
            idx.entries.push(make_entry("translate", &[], true));
        }

        let results = client.search("zzznomatch").await.unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn install_validates_invalid_manifest_json() {
        // A SkillManifest with no "name" field should fail deserialization.
        let invalid: serde_json::Value = serde_json::json!({"no_name": true});
        let parsed: Result<SkillManifest, _> = serde_json::from_value(invalid);
        assert!(parsed.is_err());
    }
}
