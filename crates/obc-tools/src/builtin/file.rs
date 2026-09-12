//! File read/write tool — read and write files on the local filesystem.
//!
//! Since 2026-09-12 it can be fenced: [`FileTool::with_roots`] limits every
//! action to the listed directories, resolving `..` and symlinks through the
//! deepest existing ancestor before comparing, so `/workspace/../secrets` and
//! a link out of the root are refused the same way. No roots means the whole
//! host, as before, and `main` says so at boot.

use crate::traits::{BlastRadius, RiskClass, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

/// Tool: read or write files on the local filesystem.
pub struct FileTool {
    /// Canonical roots the tool may touch; empty = unrestricted.
    roots: Vec<PathBuf>,
    description: String,
}

impl FileTool {
    /// Unrestricted (the whole host).
    pub fn new() -> Self {
        Self::with_roots(Vec::<PathBuf>::new())
    }

    /// Fenced to `roots`. Roots that do not exist are created so they can be
    /// canonicalised; a root that cannot be created is dropped with a warning
    /// rather than silently widening the fence.
    pub fn with_roots<P: AsRef<Path>>(roots: Vec<P>) -> Self {
        let mut canon = Vec::new();
        for r in roots {
            let expanded = shellexpand::tilde(&r.as_ref().to_string_lossy()).to_string();
            let path = PathBuf::from(expanded);
            if let Err(e) = std::fs::create_dir_all(&path) {
                tracing::warn!(root = %path.display(), error = %e, "file tool: root dropped, cannot create it");
                continue;
            }
            match std::fs::canonicalize(&path) {
                Ok(c) => canon.push(c),
                Err(e) => {
                    tracing::warn!(root = %path.display(), error = %e, "file tool: root dropped, cannot resolve it")
                }
            }
        }
        let description = if canon.is_empty() {
            "Read or write files on the local filesystem. \
             Supports reading text files, writing/overwriting text files, \
             appending to files, listing directory contents, and checking if a path exists."
                .to_string()
        } else {
            format!(
                "Read or write files under these directories only: {}. Paths outside them \
                 (including via ..) are refused. Supports reading text files, writing/overwriting \
                 text files, appending to files, listing directory contents, and checking if a \
                 path exists.",
                canon
                    .iter()
                    .map(|c| display_root(c))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        Self {
            roots: canon,
            description,
        }
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// The path an action may use, or why not. Unrestricted tools pass
    /// everything through untouched.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        let expanded = shellexpand::tilde(path).to_string();
        let requested = PathBuf::from(&expanded);
        if self.roots.is_empty() {
            return Ok(requested);
        }
        // A relative path is taken under the first root, never the process cwd.
        let absolute = if requested.is_absolute() {
            requested
        } else {
            self.roots[0].join(&requested)
        };
        // Resolve through the deepest existing ancestor so `..` and symlinks
        // are compared as what they point at; the remainder (a file about to
        // be created) may not walk upwards.
        let mut existing = absolute.clone();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        loop {
            if existing.exists() {
                break;
            }
            match (existing.file_name(), existing.parent()) {
                (Some(name), Some(parent)) => {
                    tail.push(name.to_os_string());
                    existing = parent.to_path_buf();
                }
                _ => return Err(format!("'{}' cannot be resolved", absolute.display())),
            }
        }
        let mut resolved = std::fs::canonicalize(&existing)
            .map_err(|e| format!("'{}' cannot be resolved: {e}", existing.display()))?;
        for part in tail.iter().rev() {
            match Path::new(part).components().next() {
                Some(Component::Normal(_)) => resolved.push(part),
                _ => {
                    return Err(format!(
                        "'{}' walks outside the allowed directories",
                        absolute.display()
                    ))
                }
            }
        }
        if self.roots.iter().any(|r| resolved.starts_with(r)) {
            Ok(resolved)
        } else {
            Err(format!(
                "'{}' is outside the allowed directories ({})",
                absolute.display(),
                self.roots
                    .iter()
                    .map(|c| display_root(c))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    }
}

/// A canonical path without Windows' `\\?\` verbatim prefix, for humans.
fn display_root(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

impl Default for FileTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for FileTool {
    fn name(&self) -> &str {
        "file"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "The file operation to perform.",
                    "enum": ["read", "write", "append", "list", "exists", "delete"]
                },
                "path": {
                    "type": "string",
                    "description": "The file or directory path."
                },
                "content": {
                    "type": "string",
                    "description": "The content to write or append (required for 'write' and 'append' actions)."
                }
            },
            "required": ["action", "path"]
        })
    }

    fn risk_class(&self) -> RiskClass {
        // The file tool can write/append/delete, so it is side-effecting and not
        // safely re-runnable — the self-improvement loop must quarantine learned
        // skills that use it rather than verify them by replay. Not `physical`
        // (no actuator), so the Track 0 agent gate is unaffected.
        RiskClass {
            reversible: false,
            blast: BlastRadius::Low,
            physical: false,
        }
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'action' parameter"))?
            .to_string();

        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' parameter"))?
            .to_string();

        // Expand `~`, and when roots are set, refuse anything outside them.
        let path_buf = match self.resolve(&path) {
            Ok(p) => p,
            Err(why) => return Ok(ToolResult::err(why)),
        };

        match action.as_str() {
            "read" => {
                let content = tokio::fs::read_to_string(&path_buf).await.map_err(|e| {
                    anyhow::anyhow!("Failed to read '{}': {}", path_buf.display(), e)
                })?;
                Ok(ToolResult::ok(content))
            }

            "write" => {
                let content = args
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'content' parameter for 'write'"))?;
                if let Some(parent) = path_buf.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&path_buf, content).await.map_err(|e| {
                    anyhow::anyhow!("Failed to write '{}': {}", path_buf.display(), e)
                })?;
                Ok(ToolResult::ok(format!(
                    "Wrote {} bytes to '{}'",
                    content.len(),
                    path_buf.display()
                )))
            }

            "append" => {
                let content = args
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'content' parameter for 'append'"))?;
                use tokio::io::AsyncWriteExt;
                let mut file = tokio::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path_buf)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("Failed to open '{}' for append: {}", path_buf.display(), e)
                    })?;
                file.write_all(content.as_bytes()).await?;
                Ok(ToolResult::ok(format!(
                    "Appended {} bytes to '{}'",
                    content.len(),
                    path_buf.display()
                )))
            }

            "list" => {
                let mut entries = tokio::fs::read_dir(&path_buf).await.map_err(|e| {
                    anyhow::anyhow!("Failed to list '{}': {}", path_buf.display(), e)
                })?;
                let mut names = Vec::new();
                while let Some(entry) = entries.next_entry().await? {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                    names.push(if is_dir { format!("{}/", name) } else { name });
                }
                names.sort();
                Ok(ToolResult::ok(names.join("\n")))
            }

            "exists" => {
                let exists = path_buf.exists();
                Ok(ToolResult::ok(exists.to_string()))
            }

            "delete" => {
                if path_buf.is_dir() {
                    tokio::fs::remove_dir_all(&path_buf).await.map_err(|e| {
                        anyhow::anyhow!(
                            "Failed to delete directory '{}': {}",
                            path_buf.display(),
                            e
                        )
                    })?;
                } else {
                    tokio::fs::remove_file(&path_buf).await.map_err(|e| {
                        anyhow::anyhow!("Failed to delete file '{}': {}", path_buf.display(), e)
                    })?;
                }
                Ok(ToolResult::ok(format!("Deleted '{}'", path_buf.display())))
            }

            other => Ok(ToolResult::err(format!("Unknown action: {}", other))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn write_and_read_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        let tool = FileTool::new();

        let write_result = tool
            .execute(json!({
                "action": "write",
                "path": path.to_str().unwrap(),
                "content": "Hello, Oh-Ben-Claw!"
            }))
            .await
            .unwrap();
        assert!(write_result.success);

        let read_result = tool
            .execute(json!({
                "action": "read",
                "path": path.to_str().unwrap()
            }))
            .await
            .unwrap();
        assert!(read_result.success);
        assert_eq!(read_result.output, "Hello, Oh-Ben-Claw!");
    }

    #[tokio::test]
    async fn list_directory() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        let tool = FileTool::new();

        let result = tool
            .execute(json!({
                "action": "list",
                "path": dir.path().to_str().unwrap()
            }))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("a.txt"));
        assert!(result.output.contains("b.txt"));
    }

    #[tokio::test]
    async fn exists_returns_correct_value() {
        let dir = TempDir::new().unwrap();
        let tool = FileTool::new();

        let result = tool
            .execute(json!({
                "action": "exists",
                "path": dir.path().to_str().unwrap()
            }))
            .await
            .unwrap();
        assert_eq!(result.output, "true");

        let result = tool
            .execute(json!({
                "action": "exists",
                "path": "/this/does/not/exist"
            }))
            .await
            .unwrap();
        assert_eq!(result.output, "false");
    }
}

#[cfg(test)]
mod fence_tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn a_fenced_tool_stays_inside_its_roots() {
        let root = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        std::fs::write(other.path().join("secret.txt"), "no").unwrap();
        let tool = FileTool::with_roots(vec![root.path().to_path_buf()]);
        assert_eq!(tool.roots().len(), 1);

        // inside: write into a new subdirectory, read it back, list
        let inside = root.path().join("notes").join("a.txt");
        let r = tool
            .execute(json!({"action": "write", "path": inside.to_string_lossy(), "content": "hi"}))
            .await
            .unwrap();
        assert!(r.success, "{r:?}");
        let r = tool
            .execute(json!({"action": "read", "path": inside.to_string_lossy()}))
            .await
            .unwrap();
        assert_eq!(r.output, "hi");

        // outside by absolute path
        let r = tool
            .execute(json!({"action": "read", "path": other.path().join("secret.txt").to_string_lossy()}))
            .await
            .unwrap();
        assert!(!r.success);
        assert!(
            r.error
                .as_deref()
                .unwrap_or("")
                .contains("outside the allowed directories"),
            "{r:?}"
        );

        // outside by traversal from inside
        let sneaky = root
            .path()
            .join("notes")
            .join("..")
            .join("..")
            .join("elsewhere.txt");
        let r = tool
            .execute(json!({"action": "write", "path": sneaky.to_string_lossy(), "content": "x"}))
            .await
            .unwrap();
        assert!(!r.success, "{r:?}");

        // relative paths land under the first root
        let r = tool
            .execute(json!({"action": "exists", "path": "notes/a.txt"}))
            .await
            .unwrap();
        assert_eq!(r.output, "true");

        // the description names the fence
        assert!(tool.description().contains("under these directories only"));
    }

    #[test]
    fn an_unfenced_tool_passes_paths_through() {
        let tool = FileTool::new();
        assert!(tool.roots().is_empty());
        assert_eq!(
            tool.resolve("C:/anything/at/all").unwrap(),
            PathBuf::from("C:/anything/at/all")
        );
        assert!(tool
            .description()
            .starts_with("Read or write files on the local filesystem."));
    }
}
