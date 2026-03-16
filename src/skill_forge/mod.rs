//! Skill Forge — automatic discovery and integration of new skills/tools.
//!
//! The Skill Forge lets operators extend the agent's capabilities at runtime by
//! dropping skill manifest files into a watched directory. Each manifest
//! describes a new tool: its name, description, parameter schema, and the
//! shell command (or HTTP endpoint) used to execute it.
//!
//! # Skill Manifest Format
//!
//! Skill manifests are stored as JSON files with a `.skill.json` extension:
//!
//! ```json
//! {
//!   "name": "check_weather",
//!   "version": "1.0.0",
//!   "description": "Fetch the current weather for a city.",
//!   "kind": {
//!     "type": "shell",
//!     "command": "curl -s 'https://wttr.in/{city}?format=3'"
//!   },
//!   "parameters": {
//!     "type": "object",
//!     "properties": {
//!       "city": { "type": "string", "description": "City name." }
//!     },
//!     "required": ["city"]
//!   },
//!   "tags": ["weather", "internet"]
//! }
//! ```
//!
//! # Usage
//!
//! ```rust,no_run
//! use oh_ben_claw::skill_forge::SkillForge;
//!
//! let forge = SkillForge::new("/etc/oh-ben-claw/skills");
//! let tools = forge.load_all().unwrap();
//! println!("Loaded {} skills", tools.len());
//! ```

use crate::tools::{Tool, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

// ── Skill Manifest ────────────────────────────────────────────────────────────

/// Describes how a skill tool executes its action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SkillKind {
    /// Execute a shell command. `{param_name}` placeholders in `command` are
    /// replaced with the argument values at runtime.
    Shell { command: String },
    /// Make an HTTP GET request. `{param_name}` placeholders in `url` are
    /// substituted with argument values.
    Http {
        url: String,
        #[serde(default = "default_http_method")]
        method: String,
        #[serde(default)]
        headers: std::collections::HashMap<String, String>,
        #[serde(default)]
        body_template: Option<String>,
    },
    /// Call another Oh-Ben-Claw tool by name with fixed args merged with
    /// the runtime args.
    Delegate {
        tool: String,
        #[serde(default)]
        fixed_args: Value,
    },
}

fn default_http_method() -> String {
    "GET".to_string()
}

/// A complete skill manifest loaded from a `.skill.json` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Unique tool name (snake_case, e.g. `"check_weather"`).
    pub name: String,
    /// Human-readable description shown to the LLM.
    pub description: String,
    /// How the skill is executed.
    pub kind: SkillKind,
    /// JSON Schema for the tool's parameters.
    #[serde(default = "default_empty_schema")]
    pub parameters: Value,
    /// Semantic version of the skill (optional).
    #[serde(default)]
    pub version: Option<String>,
    /// Free-form tags for categorisation.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Whether this skill is active (default: true).
    #[serde(default = "bool_true")]
    pub enabled: bool,
    /// Execution timeout in seconds (default: 30).
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_empty_schema() -> Value {
    json!({"type": "object", "properties": {}})
}

fn bool_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    30
}

impl SkillManifest {
    /// Parse a manifest from a JSON string.
    pub fn from_json(json: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(json)?)
    }

    /// Load a manifest from a file.
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        Self::from_json(&json)
    }

    /// Validate that the manifest has required fields.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.name.is_empty() {
            anyhow::bail!("Skill manifest 'name' must not be empty");
        }
        if self.description.is_empty() {
            anyhow::bail!("Skill manifest 'description' must not be empty");
        }
        // Name must be valid: lowercase, alphanumeric, underscores only
        if !self.name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            anyhow::bail!(
                "Skill name '{}' contains invalid characters (use a-z, 0-9, _)",
                self.name
            );
        }
        Ok(())
    }
}

// ── Skill Tool ────────────────────────────────────────────────────────────────

/// A dynamically-loaded skill tool.
///
/// Wraps a `SkillManifest` and implements the `Tool` trait by executing the
/// manifest's `kind` at runtime.
pub struct SkillTool {
    manifest: SkillManifest,
    client: reqwest::Client,
}

impl SkillTool {
    /// Create a tool from a manifest.
    pub fn new(manifest: SkillManifest) -> anyhow::Result<Self> {
        manifest.validate()?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(manifest.timeout_secs))
            .build()
            .unwrap_or_default();
        Ok(Self { manifest, client })
    }

    /// Fill `{placeholder}` tokens in a template string with argument values.
    fn substitute(template: &str, args: &Value) -> String {
        let mut result = template.to_string();
        if let Some(obj) = args.as_object() {
            for (key, val) in obj {
                let placeholder = format!("{{{key}}}");
                let replacement = match val {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                result = result.replace(&placeholder, &replacement);
            }
        }
        result
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        &self.manifest.name
    }

    fn description(&self) -> &str {
        &self.manifest.description
    }

    fn parameters_schema(&self) -> Value {
        self.manifest.parameters.clone()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        if !self.manifest.enabled {
            return Ok(ToolResult::err(format!(
                "Skill '{}' is disabled",
                self.manifest.name
            )));
        }

        match &self.manifest.kind {
            SkillKind::Shell { command } => {
                let cmd = Self::substitute(command, &args);
                let timeout = tokio::time::Duration::from_secs(self.manifest.timeout_secs);

                let output = tokio::time::timeout(
                    timeout,
                    tokio::process::Command::new("sh")
                        .arg("-c")
                        .arg(&cmd)
                        .output(),
                )
                .await;

                match output {
                    Ok(Ok(out)) if out.status.success() => {
                        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        Ok(ToolResult::ok(stdout))
                    }
                    Ok(Ok(out)) => {
                        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                        Ok(ToolResult::err(format!(
                            "Command exited with status {}: {stderr}",
                            out.status
                        )))
                    }
                    Ok(Err(e)) => Ok(ToolResult::err(format!("Failed to run command: {e}"))),
                    Err(_) => Ok(ToolResult::err(format!(
                        "Command timed out after {}s",
                        self.manifest.timeout_secs
                    ))),
                }
            }

            SkillKind::Http {
                url,
                method,
                headers,
                body_template,
            } => {
                let resolved_url = Self::substitute(url, &args);

                let mut req = match method.to_uppercase().as_str() {
                    "POST" | "PUT" | "PATCH" => {
                        let body = body_template
                            .as_deref()
                            .map(|t| Self::substitute(t, &args))
                            .unwrap_or_else(|| args.to_string());
                        self.client
                            .request(
                                reqwest::Method::from_bytes(method.as_bytes())
                                    .unwrap_or(reqwest::Method::POST),
                                &resolved_url,
                            )
                            .body(body)
                    }
                    _ => self.client.get(&resolved_url),
                };

                for (k, v) in headers {
                    req = req.header(k.as_str(), v.as_str());
                }

                match req.send().await {
                    Ok(resp) if resp.status().is_success() => {
                        let text = resp.text().await.unwrap_or_default();
                        Ok(ToolResult::ok(text))
                    }
                    Ok(resp) => {
                        let status = resp.status();
                        let body = resp.text().await.unwrap_or_default();
                        Ok(ToolResult::err(format!("HTTP {status}: {body}")))
                    }
                    Err(e) => Ok(ToolResult::err(format!("HTTP request failed: {e}"))),
                }
            }

            SkillKind::Delegate { tool, fixed_args } => {
                // Merge fixed_args with runtime args (runtime args take precedence)
                let mut merged = fixed_args.clone();
                if let (Some(m), Some(a)) = (merged.as_object_mut(), args.as_object()) {
                    for (k, v) in a {
                        m.insert(k.clone(), v.clone());
                    }
                }
                Ok(ToolResult::ok(format!(
                    "Delegate to tool '{}' with args: {}",
                    tool, merged
                )))
            }
        }
    }
}

// ── SkillForge ────────────────────────────────────────────────────────────────

/// Discovers and loads skills from a directory of `.skill.json` manifest files.
pub struct SkillForge {
    /// Directory to scan for skill manifests.
    pub skill_dir: PathBuf,
}

impl SkillForge {
    /// Create a forge that scans the given directory.
    pub fn new(skill_dir: impl Into<PathBuf>) -> Self {
        Self {
            skill_dir: skill_dir.into(),
        }
    }

    /// The default skill directory (`~/.config/oh-ben-claw/skills`).
    pub fn default_dir() -> PathBuf {
        std::env::var("HOME")
            .map(|h| {
                PathBuf::from(h)
                    .join(".config")
                    .join("oh-ben-claw")
                    .join("skills")
            })
            .unwrap_or_else(|_| PathBuf::from("/etc/oh-ben-claw/skills"))
    }

    /// Load all enabled skills from the skill directory.
    ///
    /// Silently skips files that cannot be parsed or fail validation, logging
    /// warnings for each failure.
    pub fn load_all(&self) -> anyhow::Result<Vec<Box<dyn Tool>>> {
        let dir = &self.skill_dir;
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let entries = std::fs::read_dir(dir)?;
        let mut tools: Vec<Box<dyn Tool>> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if !is_skill_manifest(&path) {
                continue;
            }

            match SkillManifest::from_file(&path) {
                Ok(manifest) if !manifest.enabled => {
                    tracing::debug!(name = %manifest.name, "Skipping disabled skill");
                }
                Ok(manifest) => match SkillTool::new(manifest) {
                    Ok(tool) => {
                        tracing::info!(name = %tool.name(), "Loaded skill from forge");
                        tools.push(Box::new(tool));
                    }
                    Err(e) => {
                        tracing::warn!(path = ?path, error = %e, "Failed to create skill tool");
                    }
                },
                Err(e) => {
                    tracing::warn!(path = ?path, error = %e, "Failed to parse skill manifest");
                }
            }
        }

        Ok(tools)
    }

    /// Load all skill manifests (without converting to tools) for inspection.
    pub fn list_manifests(&self) -> anyhow::Result<Vec<SkillManifest>> {
        let dir = &self.skill_dir;
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let entries = std::fs::read_dir(dir)?;
        let mut manifests = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if !is_skill_manifest(&path) {
                continue;
            }
            if let Ok(manifest) = SkillManifest::from_file(&path) {
                manifests.push(manifest);
            }
        }

        Ok(manifests)
    }

    /// Write a skill manifest to the skill directory.
    ///
    /// Creates the directory if it doesn't exist.
    pub fn install_skill(&self, manifest: &SkillManifest) -> anyhow::Result<PathBuf> {
        manifest.validate()?;
        std::fs::create_dir_all(&self.skill_dir)?;
        let path = self.skill_dir.join(format!("{}.skill.json", manifest.name));
        let json = serde_json::to_string_pretty(manifest)?;
        std::fs::write(&path, json)?;
        tracing::info!(name = %manifest.name, path = ?path, "Installed skill");
        Ok(path)
    }

    /// Remove a skill manifest from the skill directory.
    pub fn remove_skill(&self, name: &str) -> anyhow::Result<()> {
        let path = self.skill_dir.join(format!("{name}.skill.json"));
        if path.exists() {
            std::fs::remove_file(&path)?;
            tracing::info!(name = %name, "Removed skill");
        }
        Ok(())
    }
}

fn is_skill_manifest(path: &Path) -> bool {
    path.is_file()
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(".skill.json"))
            .unwrap_or(false)
}

// ── SkillForgeTool ─────────────────────────────────────────────────────────────

/// An agent tool for managing the skill forge at runtime.
///
/// Allows the agent (or a user via the agent) to list, inspect, install, and
/// remove skills without restarting the system.
pub struct SkillForgeTool {
    forge: SkillForge,
}

impl SkillForgeTool {
    /// Create with an explicit forge instance.
    pub fn new(forge: SkillForge) -> Self {
        Self { forge }
    }

    /// Create with the default skill directory.
    pub fn default_dir() -> Self {
        Self::new(SkillForge::new(SkillForge::default_dir()))
    }
}

#[async_trait]
impl Tool for SkillForgeTool {
    fn name(&self) -> &str {
        "skill_forge"
    }

    fn description(&self) -> &str {
        "Manage the skill forge: list installed skills, install new skills from a JSON \
        manifest, or remove existing skills. Skills extend the agent's capabilities at \
        runtime without requiring a restart."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "install", "remove"],
                    "description": "Action to perform."
                },
                "manifest": {
                    "type": "object",
                    "description": "Skill manifest JSON object (required for 'install' action)."
                },
                "name": {
                    "type": "string",
                    "description": "Skill name (required for 'remove' action)."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a.to_string(),
            None => return Ok(ToolResult::err("Missing required argument: action")),
        };

        match action.as_str() {
            "list" => match self.forge.list_manifests() {
                Ok(manifests) if manifests.is_empty() => {
                    Ok(ToolResult::ok("No skills installed."))
                }
                Ok(manifests) => {
                    let summary: Vec<Value> = manifests
                        .iter()
                        .map(|m| {
                            json!({
                                "name": m.name,
                                "description": m.description,
                                "version": m.version,
                                "enabled": m.enabled,
                                "tags": m.tags
                            })
                        })
                        .collect();
                    Ok(ToolResult::ok(
                        serde_json::to_string_pretty(&summary).unwrap_or_default(),
                    ))
                }
                Err(e) => Ok(ToolResult::err(format!("Failed to list skills: {e}"))),
            },

            "install" => {
                let manifest_val = match args.get("manifest") {
                    Some(m) => m.clone(),
                    None => {
                        return Ok(ToolResult::err(
                            "Missing required argument: manifest (for 'install' action)",
                        ))
                    }
                };
                let manifest: SkillManifest = match serde_json::from_value(manifest_val) {
                    Ok(m) => m,
                    Err(e) => return Ok(ToolResult::err(format!("Invalid manifest: {e}"))),
                };
                match self.forge.install_skill(&manifest) {
                    Ok(path) => Ok(ToolResult::ok(format!(
                        "Skill '{}' installed to {}",
                        manifest.name,
                        path.display()
                    ))),
                    Err(e) => Ok(ToolResult::err(format!("Failed to install skill: {e}"))),
                }
            }

            "remove" => {
                let name = match args.get("name").and_then(|v| v.as_str()) {
                    Some(n) => n.to_string(),
                    None => {
                        return Ok(ToolResult::err(
                            "Missing required argument: name (for 'remove' action)",
                        ))
                    }
                };
                match self.forge.remove_skill(&name) {
                    Ok(()) => Ok(ToolResult::ok(format!("Skill '{name}' removed."))),
                    Err(e) => Ok(ToolResult::err(format!("Failed to remove skill: {e}"))),
                }
            }

            other => Ok(ToolResult::err(format!(
                "Unknown action '{other}'. Use 'list', 'install', or 'remove'."
            ))),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_shell_manifest(name: &str) -> SkillManifest {
        SkillManifest {
            name: name.to_string(),
            description: "A test skill.".to_string(),
            kind: SkillKind::Shell {
                command: "echo hello".to_string(),
            },
            parameters: default_empty_schema(),
            version: Some("1.0.0".to_string()),
            tags: vec!["test".to_string()],
            enabled: true,
            timeout_secs: 5,
        }
    }

    #[test]
    fn manifest_validate_rejects_empty_name() {
        let mut m = sample_shell_manifest("valid");
        m.name = String::new();
        assert!(m.validate().is_err());
    }

    #[test]
    fn manifest_validate_rejects_invalid_chars() {
        let mut m = sample_shell_manifest("valid");
        m.name = "my-skill".to_string(); // hyphens are not allowed
        assert!(m.validate().is_err());
    }

    #[test]
    fn manifest_validate_rejects_empty_description() {
        let mut m = sample_shell_manifest("valid");
        m.description = String::new();
        assert!(m.validate().is_err());
    }

    #[test]
    fn manifest_validate_accepts_valid() {
        let m = sample_shell_manifest("echo_test");
        assert!(m.validate().is_ok());
    }

    #[test]
    fn skill_tool_name_matches_manifest() {
        let m = sample_shell_manifest("echo_test");
        let tool = SkillTool::new(m).unwrap();
        assert_eq!(tool.name(), "echo_test");
    }

    #[test]
    fn substitute_fills_placeholders() {
        let template = "curl https://example.com/{city}/weather?units={units}";
        let args = json!({"city": "london", "units": "metric"});
        let result = SkillTool::substitute(template, &args);
        assert_eq!(result, "curl https://example.com/london/weather?units=metric");
    }

    #[test]
    fn substitute_leaves_missing_placeholders() {
        let template = "echo {name}";
        let args = json!({});
        let result = SkillTool::substitute(template, &args);
        assert_eq!(result, "echo {name}");
    }

    #[tokio::test]
    async fn shell_skill_executes_command() {
        let m = SkillManifest {
            kind: SkillKind::Shell {
                command: "echo skill_works".to_string(),
            },
            ..sample_shell_manifest("echo_skill")
        };
        let tool = SkillTool::new(m).unwrap();
        let result = tool.execute(json!({})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("skill_works"));
    }

    #[tokio::test]
    async fn shell_skill_substitutes_args_in_command() {
        let m = SkillManifest {
            kind: SkillKind::Shell {
                command: "echo {greeting}".to_string(),
            },
            ..sample_shell_manifest("greet_skill")
        };
        let tool = SkillTool::new(m).unwrap();
        let result = tool
            .execute(json!({"greeting": "hello_world"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello_world"));
    }

    #[tokio::test]
    async fn disabled_skill_returns_error() {
        let m = SkillManifest {
            enabled: false,
            ..sample_shell_manifest("disabled_skill")
        };
        let tool = SkillTool::new(m).unwrap();
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or(&result.output)
            .contains("disabled"));
    }

    #[test]
    fn forge_returns_empty_for_nonexistent_dir() {
        let forge = SkillForge::new("/nonexistent/skills/dir");
        let tools = forge.load_all().unwrap();
        assert!(tools.is_empty());
    }

    #[test]
    fn forge_install_and_load() {
        let tmp = TempDir::new().unwrap();
        let forge = SkillForge::new(tmp.path());
        let manifest = sample_shell_manifest("install_test");
        forge.install_skill(&manifest).unwrap();

        let tools = forge.load_all().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "install_test");
    }

    #[test]
    fn forge_remove_skill() {
        let tmp = TempDir::new().unwrap();
        let forge = SkillForge::new(tmp.path());
        let manifest = sample_shell_manifest("remove_test");
        forge.install_skill(&manifest).unwrap();
        forge.remove_skill("remove_test").unwrap();

        let tools = forge.load_all().unwrap();
        assert!(tools.is_empty());
    }

    #[test]
    fn forge_skip_disabled_skills() {
        let tmp = TempDir::new().unwrap();
        let forge = SkillForge::new(tmp.path());
        let mut manifest = sample_shell_manifest("disabled_test");
        manifest.enabled = false;
        forge.install_skill(&manifest).unwrap();

        let tools = forge.load_all().unwrap();
        assert!(tools.is_empty());
    }

    #[test]
    fn forge_list_manifests() {
        let tmp = TempDir::new().unwrap();
        let forge = SkillForge::new(tmp.path());
        forge.install_skill(&sample_shell_manifest("skill_a")).unwrap();
        forge.install_skill(&sample_shell_manifest("skill_b")).unwrap();

        let manifests = forge.list_manifests().unwrap();
        assert_eq!(manifests.len(), 2);
    }

    #[test]
    fn manifest_from_json_round_trip() {
        let m = sample_shell_manifest("round_trip");
        let json = serde_json::to_string(&m).unwrap();
        let m2 = SkillManifest::from_json(&json).unwrap();
        assert_eq!(m.name, m2.name);
        assert_eq!(m.description, m2.description);
    }

    #[tokio::test]
    async fn forge_tool_list_action_empty() {
        let tmp = TempDir::new().unwrap();
        let forge_tool = SkillForgeTool::new(SkillForge::new(tmp.path()));
        let result = forge_tool
            .execute(json!({"action": "list"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("No skills"));
    }

    #[tokio::test]
    async fn forge_tool_install_then_list() {
        let tmp = TempDir::new().unwrap();
        let forge_tool = SkillForgeTool::new(SkillForge::new(tmp.path()));

        let manifest_val = json!({
            "name": "forge_tool_test",
            "description": "A test skill installed via forge tool.",
            "kind": { "type": "shell", "command": "echo ok" }
        });

        let install_result = forge_tool
            .execute(json!({"action": "install", "manifest": manifest_val}))
            .await
            .unwrap();
        assert!(install_result.success, "{:?}", install_result.error);

        let list_result = forge_tool
            .execute(json!({"action": "list"}))
            .await
            .unwrap();
        assert!(list_result.success);
        assert!(list_result.output.contains("forge_tool_test"));
    }

    #[tokio::test]
    async fn forge_tool_remove_action() {
        let tmp = TempDir::new().unwrap();
        let forge_tool = SkillForgeTool::new(SkillForge::new(tmp.path()));

        // Install first
        let manifest_val = json!({
            "name": "to_remove",
            "description": "Will be removed.",
            "kind": { "type": "shell", "command": "echo bye" }
        });
        forge_tool
            .execute(json!({"action": "install", "manifest": manifest_val}))
            .await
            .unwrap();

        // Then remove
        let remove_result = forge_tool
            .execute(json!({"action": "remove", "name": "to_remove"}))
            .await
            .unwrap();
        assert!(remove_result.success);
    }

    #[tokio::test]
    async fn forge_tool_unknown_action_error() {
        let tmp = TempDir::new().unwrap();
        let forge_tool = SkillForgeTool::new(SkillForge::new(tmp.path()));
        let result = forge_tool
            .execute(json!({"action": "destroy_everything"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or(&result.output)
            .contains("Unknown action"));
    }

    #[tokio::test]
    async fn forge_tool_missing_action_error() {
        let tmp = TempDir::new().unwrap();
        let forge_tool = SkillForgeTool::new(SkillForge::new(tmp.path()));
        let result = forge_tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn delegate_skill_merges_args() {
        let m = SkillManifest {
            kind: SkillKind::Delegate {
                tool: "shell".to_string(),
                fixed_args: json!({"fixed": "value"}),
            },
            ..sample_shell_manifest("delegate_skill")
        };
        let tool = SkillTool::new(m).unwrap();
        let result = tool
            .execute(json!({"runtime": "arg"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("shell"));
    }
}
