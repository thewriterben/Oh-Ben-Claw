//! File read/write tool — read and write files on the local filesystem.

use crate::tools::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};

/// Tool: read or write files on the local filesystem.
pub struct FileTool;

impl FileTool {
    pub fn new() -> Self {
        Self
    }
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
        "Read or write files on the local filesystem. \
         Supports reading text files, writing/overwriting text files, \
         appending to files, listing directory contents, and checking if a path exists."
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

        // Expand ~ to home directory
        let expanded = shellexpand::tilde(&path).to_string();
        let path_buf = std::path::PathBuf::from(&expanded);

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
                    names.push(if is_dir {
                        format!("{}/", name)
                    } else {
                        name
                    });
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
                        anyhow::anyhow!("Failed to delete directory '{}': {}", path_buf.display(), e)
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
