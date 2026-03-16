//! RAG (Retrieval-Augmented Generation) pipeline for hardware datasheets.
//!
//! This module provides a simple keyword-based search index over local Markdown
//! and text files, specifically designed for hardware datasheet retrieval.

use crate::tools::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// A chunk of a hardware datasheet or documentation file.
#[derive(Debug, Clone)]
pub struct DatasheetChunk {
    /// The board this chunk relates to (derived from the filename).
    pub board: Option<String>,
    /// The source file path.
    pub source: String,
    /// The text content of this chunk.
    pub content: String,
}

/// A map from pin alias name to GPIO pin number.
pub type PinAliases = HashMap<String, u32>;

/// An in-memory index of datasheet chunks and pin aliases.
pub struct RagIndex {
    chunks: Vec<DatasheetChunk>,
    aliases: HashMap<String, PinAliases>,
}

impl RagIndex {
    /// Create an empty `RagIndex`.
    pub fn new() -> Self {
        Self { chunks: Vec::new(), aliases: HashMap::new() }
    }

    /// Load all `.md` and `.txt` files from the given directory, chunk them,
    /// and extract any pin alias tables.
    pub fn load_dir(path: &Path) -> anyhow::Result<Self> {
        let mut index = Self::new();

        if !path.exists() {
            return Ok(index);
        }

        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let p = entry.path();
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext != "md" && ext != "txt" {
                continue;
            }

            let content = match std::fs::read_to_string(&p) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let board = p
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string());

            let source = p.to_string_lossy().to_string();

            // Extract pin aliases before chunking
            if let Some(ref b) = board {
                let aliases = parse_pin_aliases(&content);
                if !aliases.is_empty() {
                    index.aliases.insert(b.clone(), aliases);
                }
            }

            // Chunk into ~500 character pieces, splitting on paragraph breaks.
            // We ensure `end > start` on every iteration to guarantee termination.
            let mut start = 0;
            while start < content.len() {
                let raw_end = if start + 500 >= content.len() {
                    content.len()
                } else {
                    // Try to break at a paragraph boundary
                    let window = &content[start..start + 500];
                    if let Some(pos) = window.rfind("\n\n") {
                        start + pos + 2
                    } else if let Some(pos) = window.rfind('\n') {
                        start + pos + 1
                    } else {
                        start + 500
                    }
                };
                // Guard: clamp to content.len() first (handles start+1 > len),
                // then ensure we advance by at least one byte to prevent infinite loops.
                let end = raw_end.min(content.len()).max(start + 1).min(content.len());

                let chunk_text = content[start..end].trim().to_string();
                if !chunk_text.is_empty() {
                    index.chunks.push(DatasheetChunk {
                        board: board.clone(),
                        source: source.clone(),
                        content: chunk_text,
                    });
                }
                start = end;
            }
        }

        Ok(index)
    }

    /// Search the index for chunks matching a keyword query.
    ///
    /// Scores each chunk by the number of query words it contains (case-insensitive).
    /// Optionally filters by board name. Returns the top `top_k` results.
    pub fn search(&self, query: &str, board: Option<&str>, top_k: usize) -> Vec<&DatasheetChunk> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .collect();

        let mut scored: Vec<(usize, &DatasheetChunk)> = self
            .chunks
            .iter()
            .filter(|c| board.map_or(true, |b| c.board.as_deref() == Some(b)))
            .map(|c| {
                let lower = c.content.to_lowercase();
                let score = words.iter().filter(|w| lower.contains(w.as_str())).count();
                (score, c)
            })
            .filter(|(score, _)| *score > 0)
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().take(top_k).map(|(_, c)| c).collect()
    }

    /// Look up a pin number by its alias for a given board.
    pub fn lookup_pin(&self, board: &str, alias: &str) -> Option<u32> {
        self.aliases.get(board)?.get(alias).copied()
    }

    /// Return a [`Tool`] that exposes this index as a `datasheet_search` tool.
    pub fn as_tool(self) -> Box<dyn Tool> {
        Box::new(DatasheetSearchTool { index: self })
    }
}

impl Default for RagIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse pin alias definitions from a datasheet's content.
///
/// Looks for a `## Pin Aliases` section and extracts:
/// - Lines of the form `alias: pin` (e.g. `LED: 13`)
/// - Markdown table rows with two columns: `| alias | pin |`
fn parse_pin_aliases(content: &str) -> PinAliases {
    let mut aliases = PinAliases::new();
    let mut in_section = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("## Pin Aliases") {
            in_section = true;
            continue;
        }
        // A new `##` section ends the pin aliases section
        if in_section && trimmed.starts_with("## ") && !trimmed.starts_with("## Pin Aliases") {
            break;
        }

        if !in_section {
            continue;
        }

        // Try `alias: pin` format
        if let Some((alias, pin_str)) = trimmed.split_once(':') {
            if let Ok(pin) = pin_str.trim().parse::<u32>() {
                aliases.insert(alias.trim().to_string(), pin);
                continue;
            }
        }

        // Try Markdown table row `| alias | pin |`
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let cols: Vec<&str> = trimmed
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            if cols.len() == 2 {
                if let Ok(pin) = cols[1].parse::<u32>() {
                    aliases.insert(cols[0].to_string(), pin);
                }
            }
        }
    }

    aliases
}

// ── Tool wrapper ──────────────────────────────────────────────────────────────

struct DatasheetSearchTool {
    index: RagIndex,
}

#[async_trait]
impl Tool for DatasheetSearchTool {
    fn name(&self) -> &str {
        "datasheet_search"
    }

    fn description(&self) -> &str {
        "Search hardware datasheets and documentation for technical information."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query."
                },
                "board": {
                    "type": "string",
                    "description": "Optional board name filter."
                },
                "top_k": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default 5).",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let query = args["query"].as_str().unwrap_or("").to_string();
        let board = args["board"].as_str();
        let top_k = args["top_k"].as_u64().unwrap_or(5) as usize;

        if query.is_empty() {
            return Ok(ToolResult::err("query is required"));
        }

        let results = self.index.search(&query, board, top_k);
        if results.is_empty() {
            return Ok(ToolResult::ok("No results found."));
        }

        let output = results
            .iter()
            .enumerate()
            .map(|(i, c)| {
                format!(
                    "[{}] Source: {}\n{}",
                    i + 1,
                    c.source,
                    c.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");

        Ok(ToolResult::ok(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_pin_aliases_colon_format() {
        let content = "## Pin Aliases\nLED: 13\nBUTTON: 2\n";
        let aliases = parse_pin_aliases(content);
        assert_eq!(aliases.get("LED"), Some(&13));
        assert_eq!(aliases.get("BUTTON"), Some(&2));
    }

    #[test]
    fn parse_pin_aliases_table_format() {
        let content = "## Pin Aliases\n| LED | 13 |\n| BUTTON | 2 |\n";
        let aliases = parse_pin_aliases(content);
        assert_eq!(aliases.get("LED"), Some(&13));
        assert_eq!(aliases.get("BUTTON"), Some(&2));
    }

    #[test]
    fn parse_pin_aliases_empty_when_no_section() {
        let content = "No pin aliases here.\n";
        let aliases = parse_pin_aliases(content);
        assert!(aliases.is_empty());
    }

    #[test]
    fn load_dir_nonexistent_returns_empty() {
        let index = RagIndex::load_dir(Path::new("/nonexistent/path/xyz")).unwrap();
        assert!(index.chunks.is_empty());
    }

    #[test]
    fn search_finds_matching_chunk() {
        let mut index = RagIndex::new();
        index.chunks.push(DatasheetChunk {
            board: Some("rpi4".to_string()),
            source: "test.md".to_string(),
            content: "The GPIO pins on Raspberry Pi 4 support I2C and SPI.".to_string(),
        });
        index.chunks.push(DatasheetChunk {
            board: Some("rpi4".to_string()),
            source: "test.md".to_string(),
            content: "Power supply requirements: 5V 3A USB-C.".to_string(),
        });

        let results = index.search("GPIO I2C", None, 5);
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("GPIO"));
    }

    #[test]
    fn search_filters_by_board() {
        let mut index = RagIndex::new();
        index.chunks.push(DatasheetChunk {
            board: Some("rpi4".to_string()),
            source: "rpi4.md".to_string(),
            content: "GPIO pins on rpi4".to_string(),
        });
        index.chunks.push(DatasheetChunk {
            board: Some("esp32".to_string()),
            source: "esp32.md".to_string(),
            content: "GPIO pins on esp32".to_string(),
        });

        let results = index.search("GPIO", Some("rpi4"), 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].board.as_deref(), Some("rpi4"));
    }

    #[test]
    fn lookup_pin_returns_correct_number() {
        let mut index = RagIndex::new();
        let mut aliases = PinAliases::new();
        aliases.insert("LED".to_string(), 13);
        index.aliases.insert("rpi4".to_string(), aliases);

        assert_eq!(index.lookup_pin("rpi4", "LED"), Some(13));
        assert_eq!(index.lookup_pin("rpi4", "UNKNOWN"), None);
        assert_eq!(index.lookup_pin("esp32", "LED"), None);
    }

    #[test]
    fn load_dir_chunks_files() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("testboard.md");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(f, "## Pin Aliases\nLED: 13\n\nSome content about the board GPIO.").unwrap();

        let index = RagIndex::load_dir(dir.path()).unwrap();
        assert!(!index.chunks.is_empty());
        assert_eq!(index.lookup_pin("testboard", "LED"), Some(13));
    }
}
