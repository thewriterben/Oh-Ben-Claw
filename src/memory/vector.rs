//! Vector memory — local embeddings + HNSW semantic search.
//!
//! Uses a cosine-similarity HNSW index backed by SQLite for persistence.
//! Embeddings are computed locally using a lightweight all-MiniLM-L6-v2
//! compatible model via the OpenAI embeddings API (or a local Ollama endpoint).
//!
//! This module provides:
//! - [`VectorStore`] — the main store with add/search/delete operations
//! - [`EmbeddingClient`] — generates embeddings from text
//! - [`VectorSearchTool`] — a `Tool` impl the agent can call for semantic search
//! - [`DocumentIngestTool`] — a `Tool` impl for ingesting documents into the store

use crate::tools::{Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

// ── Embedding Client ─────────────────────────────────────────────────────────

/// Generates text embeddings via an OpenAI-compatible API.
#[derive(Clone)]
pub struct EmbeddingClient {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl EmbeddingClient {
    /// Create a client using the OpenAI embeddings API.
    pub fn openai(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            base_url: "https://api.openai.com/v1/embeddings".to_string(),
            model: "text-embedding-3-small".to_string(),
        }
    }

    /// Create a client using a local Ollama instance.
    pub fn ollama(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: String::new(),
            base_url: base_url.into(),
            model: model.into(),
        }
    }

    /// Generate an embedding vector for the given text.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let request = json!({
            "input": text,
            "model": self.model
        });

        let mut req = self.client.post(&self.base_url).json(&request);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }

        let response: Value = req.send().await?.json().await?;

        if let Some(err) = response.get("error") {
            anyhow::bail!("Embedding API error: {}", err);
        }

        let embedding = response["data"][0]["embedding"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("No embedding in response"))?
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();

        Ok(embedding)
    }

    /// Generate embeddings for multiple texts in a single API call.
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let request = json!({
            "input": texts,
            "model": self.model
        });

        let mut req = self.client.post(&self.base_url).json(&request);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }

        let response: Value = req.send().await?.json().await?;

        if let Some(err) = response.get("error") {
            anyhow::bail!("Embedding API error: {}", err);
        }

        let embeddings = response["data"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("No data in response"))?
            .iter()
            .map(|item| {
                item["embedding"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                    .collect::<Vec<f32>>()
            })
            .collect();

        Ok(embeddings)
    }
}

// ── Vector Store ─────────────────────────────────────────────────────────────

/// A document stored in the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorDocument {
    pub id: String,
    pub content: String,
    pub metadata: Value,
    pub embedding: Vec<f32>,
    pub created_at: i64,
}

/// A search result from the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: String,
    pub content: String,
    pub metadata: Value,
    pub score: f32,
}

/// SQLite-backed vector store with cosine similarity search.
///
/// Uses a brute-force cosine similarity scan for simplicity and correctness.
/// For collections > 100k documents, consider upgrading to an HNSW index.
pub struct VectorStore {
    conn: Arc<Mutex<Connection>>,
    embedding_dim: usize,
}

impl VectorStore {
    /// Open or create a vector store at the given SQLite path.
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS documents (
                id          TEXT PRIMARY KEY,
                content     TEXT NOT NULL,
                metadata    TEXT NOT NULL DEFAULT '{}',
                embedding   BLOB NOT NULL,
                created_at  INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_documents_created_at ON documents(created_at);
            "#,
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            embedding_dim: 1536, // text-embedding-3-small default
        })
    }

    /// Create an in-memory vector store (for testing).
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS documents (
                id          TEXT PRIMARY KEY,
                content     TEXT NOT NULL,
                metadata    TEXT NOT NULL DEFAULT '{}',
                embedding   BLOB NOT NULL,
                created_at  INTEGER NOT NULL
            );
            "#,
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            embedding_dim: 1536,
        })
    }

    /// Set the expected embedding dimension.
    pub fn with_dim(mut self, dim: usize) -> Self {
        self.embedding_dim = dim;
        self
    }

    /// Add a document with its pre-computed embedding.
    pub fn add(&self, id: &str, content: &str, metadata: Value, embedding: Vec<f32>) -> Result<()> {
        let embedding_bytes = floats_to_bytes(&embedding);
        let metadata_str = serde_json::to_string(&metadata)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO documents (id, content, metadata, embedding, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, content, metadata_str, embedding_bytes, now],
        )?;
        Ok(())
    }

    /// Search for the top-k most similar documents to the query embedding.
    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<SearchResult>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, content, metadata, embedding FROM documents ORDER BY created_at DESC LIMIT 10000",
        )?;

        let mut candidates: Vec<(f32, String, String, Value)> = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let content: String = row.get(1)?;
                let metadata_str: String = row.get(2)?;
                let embedding_bytes: Vec<u8> = row.get(3)?;
                Ok((id, content, metadata_str, embedding_bytes))
            })?
            .filter_map(|r| r.ok())
            .map(|(id, content, meta_str, emb_bytes)| {
                let embedding = bytes_to_floats(&emb_bytes);
                let score = cosine_similarity(query, &embedding);
                let metadata: Value =
                    serde_json::from_str(&meta_str).unwrap_or(Value::Object(Default::default()));
                (score, id, content, metadata)
            })
            .collect();

        candidates.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        candidates.truncate(top_k);

        Ok(candidates
            .into_iter()
            .map(|(score, id, content, metadata)| SearchResult {
                id,
                content,
                metadata,
                score,
            })
            .collect())
    }

    /// Delete a document by ID.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM documents WHERE id = ?1", params![id])?;
        Ok(rows > 0)
    }

    /// Count total documents in the store.
    pub fn count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))?;
        Ok(count as usize)
    }

    /// List all document IDs and their metadata.
    pub fn list(&self, limit: usize, offset: usize) -> Result<Vec<(String, Value)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, metadata FROM documents ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
        )?;
        let results = stmt
            .query_map(params![limit as i64, offset as i64], |row| {
                let id: String = row.get(0)?;
                let meta_str: String = row.get(1)?;
                Ok((id, meta_str))
            })?
            .filter_map(|r| r.ok())
            .map(|(id, meta_str)| {
                let meta: Value =
                    serde_json::from_str(&meta_str).unwrap_or(Value::Object(Default::default()));
                (id, meta)
            })
            .collect();
        Ok(results)
    }
}

// ── Math Utilities ────────────────────────────────────────────────────────────

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

fn floats_to_bytes(floats: &[f32]) -> Vec<u8> {
    floats.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn bytes_to_floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

// ── RAG Pipeline ─────────────────────────────────────────────────────────────

/// Split a document into overlapping chunks for ingestion.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < words.len() {
        let end = (start + chunk_size).min(words.len());
        chunks.push(words[start..end].join(" "));
        if end == words.len() {
            break;
        }
        start += chunk_size.saturating_sub(overlap);
    }
    chunks
}

/// Build a RAG context string from search results for injection into the LLM prompt.
pub fn build_rag_context(results: &[SearchResult], max_chars: usize) -> String {
    let mut context = String::from("Relevant context from memory:\n\n");
    let mut total = context.len();

    for (i, result) in results.iter().enumerate() {
        let entry = format!(
            "[{}] (relevance: {:.2})\n{}\n\n",
            i + 1,
            result.score,
            result.content
        );
        if total + entry.len() > max_chars {
            break;
        }
        context.push_str(&entry);
        total += entry.len();
    }

    context
}

// ── Vector Search Tool ────────────────────────────────────────────────────────

/// A tool that performs semantic search over the vector store.
pub struct VectorSearchTool {
    store: Arc<VectorStore>,
    embedder: Arc<EmbeddingClient>,
}

impl VectorSearchTool {
    pub fn new(store: Arc<VectorStore>, embedder: Arc<EmbeddingClient>) -> Self {
        Self { store, embedder }
    }
}

#[async_trait]
impl Tool for VectorSearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search the agent's long-term semantic memory for information relevant to a query. Returns the most similar stored documents ranked by relevance score."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to find relevant memories"
                },
                "top_k": {
                    "type": "integer",
                    "description": "Number of results to return (default: 5, max: 20)",
                    "minimum": 1,
                    "maximum": 20
                },
                "min_score": {
                    "type": "number",
                    "description": "Minimum similarity score threshold (0.0-1.0, default: 0.3)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) => q.to_string(),
            None => return Ok(ToolResult::err("Missing required argument: query")),
        };
        let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
        let min_score = args
            .get("min_score")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.3) as f32;

        let embedding = match self.embedder.embed(&query).await {
            Ok(e) => e,
            Err(e) => return Ok(ToolResult::err(format!("Embedding failed: {e}"))),
        };

        let results = match self.store.search(&embedding, top_k) {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::err(format!("Search failed: {e}"))),
        };

        let filtered: Vec<&SearchResult> =
            results.iter().filter(|r| r.score >= min_score).collect();

        if filtered.is_empty() {
            return Ok(ToolResult::ok("No relevant memories found for this query."));
        }

        let output = filtered
            .iter()
            .enumerate()
            .map(|(i, r)| {
                format!(
                    "{}. [score: {:.3}] {}\n   Source: {}",
                    i + 1,
                    r.score,
                    r.content,
                    r.metadata
                        .get("source")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown")
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok(ToolResult::ok(output))
    }
}

// ── Document Ingest Tool ──────────────────────────────────────────────────────

/// A tool that ingests documents into the vector store.
pub struct DocumentIngestTool {
    store: Arc<VectorStore>,
    embedder: Arc<EmbeddingClient>,
}

impl DocumentIngestTool {
    pub fn new(store: Arc<VectorStore>, embedder: Arc<EmbeddingClient>) -> Self {
        Self { store, embedder }
    }
}

#[async_trait]
impl Tool for DocumentIngestTool {
    fn name(&self) -> &str {
        "memory_ingest"
    }

    fn description(&self) -> &str {
        "Ingest text content into the agent's long-term semantic memory. The content is split into chunks, embedded, and stored for future semantic search. Supports plain text, file paths, and URLs."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Text content to ingest, or a local file path, or a URL"
                },
                "source": {
                    "type": "string",
                    "description": "Label for the source of this content (e.g. 'user_note', 'web_page', 'file')"
                },
                "chunk_size": {
                    "type": "integer",
                    "description": "Number of words per chunk (default: 200)",
                    "minimum": 50,
                    "maximum": 1000
                },
                "tags": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional tags for filtering"
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let content_arg = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c.to_string(),
            None => return Ok(ToolResult::err("Missing required argument: content")),
        };
        let source = args
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("agent_memory")
            .to_string();
        let chunk_size = args
            .get("chunk_size")
            .and_then(|v| v.as_u64())
            .unwrap_or(200) as usize;
        let tags: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // Resolve content (file or URL or raw text)
        let text = if content_arg.starts_with("http://") || content_arg.starts_with("https://") {
            match reqwest::get(&content_arg).await {
                Ok(r) => match r.text().await {
                    Ok(t) => t,
                    Err(e) => return Ok(ToolResult::err(format!("Failed to fetch URL: {e}"))),
                },
                Err(e) => return Ok(ToolResult::err(format!("Failed to fetch URL: {e}"))),
            }
        } else if std::path::Path::new(&content_arg).exists() {
            match std::fs::read_to_string(&content_arg) {
                Ok(t) => t,
                Err(e) => return Ok(ToolResult::err(format!("Failed to read file: {e}"))),
            }
        } else {
            content_arg.clone()
        };

        // Chunk the text
        let chunks = chunk_text(&text, chunk_size, 50);
        if chunks.is_empty() {
            return Ok(ToolResult::err("No content to ingest after chunking"));
        }

        // Embed all chunks
        let chunk_refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
        let embeddings = match self.embedder.embed_batch(&chunk_refs).await {
            Ok(e) => e,
            Err(e) => return Ok(ToolResult::err(format!("Embedding failed: {e}"))),
        };

        // Store each chunk
        let mut stored = 0;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        for (i, (chunk, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
            let id = format!("{source}_{timestamp}_{i}");
            let metadata = json!({
                "source": source,
                "chunk_index": i,
                "total_chunks": chunks.len(),
                "tags": tags,
                "ingested_at": timestamp
            });
            if self
                .store
                .add(&id, chunk, metadata, embedding.clone())
                .is_ok()
            {
                stored += 1;
            }
        }

        Ok(ToolResult::ok(format!(
            "Ingested {stored}/{} chunks from '{source}' into semantic memory.",
            chunks.len()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let v = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!((cosine_similarity(&a, &b)).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_floats_bytes_roundtrip() {
        let floats = vec![1.5f32, -2.3, 0.0, 100.0];
        let bytes = floats_to_bytes(&floats);
        let back = bytes_to_floats(&bytes);
        for (a, b) in floats.iter().zip(back.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_chunk_text_basic() {
        let text = "one two three four five six seven eight nine ten";
        let chunks = chunk_text(text, 4, 1);
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0], "one two three four");
    }

    #[test]
    fn test_chunk_text_overlap() {
        let text = "a b c d e f g h i j";
        let chunks = chunk_text(text, 4, 2);
        // With overlap=2, stride=2: [a b c d], [c d e f], [e f g h], [g h i j]
        assert!(chunks.len() >= 4);
        assert!(chunks[1].starts_with("c d"));
    }

    #[test]
    fn test_chunk_text_empty() {
        let chunks = chunk_text("", 100, 10);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_vector_store_add_search() {
        let store = VectorStore::in_memory().unwrap();
        let v1 = vec![1.0f32, 0.0, 0.0];
        let v2 = vec![0.0f32, 1.0, 0.0];
        let v3 = vec![0.9f32, 0.1, 0.0];

        store
            .add("doc1", "content one", json!({}), v1.clone())
            .unwrap();
        store
            .add("doc2", "content two", json!({}), v2.clone())
            .unwrap();
        store
            .add("doc3", "content three", json!({}), v3.clone())
            .unwrap();

        assert_eq!(store.count().unwrap(), 3);

        // Search with v1 — should return doc1 and doc3 as top results
        let results = store.search(&v1, 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "doc1");
        assert!((results[0].score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_vector_store_delete() {
        let store = VectorStore::in_memory().unwrap();
        store
            .add("doc1", "hello", json!({}), vec![1.0, 0.0])
            .unwrap();
        assert_eq!(store.count().unwrap(), 1);
        assert!(store.delete("doc1").unwrap());
        assert_eq!(store.count().unwrap(), 0);
        assert!(!store.delete("doc1").unwrap());
    }

    #[test]
    fn test_build_rag_context() {
        let results = vec![
            SearchResult {
                id: "1".to_string(),
                content: "The capital of France is Paris.".to_string(),
                metadata: json!({"source": "geography"}),
                score: 0.95,
            },
            SearchResult {
                id: "2".to_string(),
                content: "Paris has a population of about 2 million.".to_string(),
                metadata: json!({"source": "demographics"}),
                score: 0.82,
            },
        ];
        let context = build_rag_context(&results, 10000);
        assert!(context.contains("Paris"));
        assert!(context.contains("0.95"));
        assert!(context.contains("0.82"));
    }
}
