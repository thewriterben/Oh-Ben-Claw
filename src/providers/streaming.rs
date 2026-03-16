//! Streaming LLM response support.
//!
//! Provides token-by-token streaming from LLM providers via Server-Sent Events
//! and async channels. Compatible with OpenAI, Anthropic, Ollama, and OpenRouter.

use crate::providers::{Message, ToolCall};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// A single chunk from a streaming LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    /// Incremental text token (may be empty for tool-call chunks).
    pub delta: String,
    /// Whether this is the final chunk.
    pub done: bool,
    /// Tool call being streamed (if any).
    pub tool_call: Option<PartialToolCall>,
    /// Finish reason when done=true.
    pub finish_reason: Option<String>,
}

/// A partially-assembled tool call (streamed argument fragments).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PartialToolCall {
    pub index: usize,
    pub id: String,
    pub name: String,
    /// JSON arguments accumulated so far.
    pub arguments: String,
}

impl PartialToolCall {
    /// Convert to a complete ToolCall once streaming is done.
    pub fn into_tool_call(self) -> ToolCall {
        ToolCall {
            id: self.id,
            name: self.name,
            arguments: self.arguments,
        }
    }
}

/// A streaming response handle — receive chunks via the channel.
pub struct StreamingResponse {
    pub rx: mpsc::Receiver<StreamChunk>,
}

impl StreamingResponse {
    pub fn new(rx: mpsc::Receiver<StreamChunk>) -> Self {
        Self { rx }
    }

    /// Collect all chunks into a complete text string (discards tool calls).
    pub async fn collect_text(mut self) -> String {
        let mut text = String::new();
        while let Some(chunk) = self.rx.recv().await {
            text.push_str(&chunk.delta);
            if chunk.done {
                break;
            }
        }
        text
    }

    /// Collect all chunks into text and assembled tool calls.
    pub async fn collect_all(mut self) -> (String, Vec<ToolCall>) {
        let mut text = String::new();
        let mut partials: std::collections::HashMap<usize, PartialToolCall> =
            std::collections::HashMap::new();

        while let Some(chunk) = self.rx.recv().await {
            text.push_str(&chunk.delta);
            if let Some(ptc) = chunk.tool_call {
                let entry = partials.entry(ptc.index).or_default();
                if !ptc.id.is_empty() {
                    entry.id = ptc.id;
                }
                if !ptc.name.is_empty() {
                    entry.name = ptc.name;
                }
                entry.arguments.push_str(&ptc.arguments);
                entry.index = ptc.index;
            }
            if chunk.done {
                break;
            }
        }

        let tool_calls = partials
            .into_values()
            .filter(|p| !p.name.is_empty())
            .map(|p| p.into_tool_call())
            .collect();

        (text, tool_calls)
    }
}

/// Parse an OpenAI-compatible SSE stream line into a StreamChunk.
pub fn parse_openai_sse_line(line: &str) -> Option<StreamChunk> {
    let line = line.trim();
    if line == "data: [DONE]" {
        return Some(StreamChunk {
            delta: String::new(),
            done: true,
            tool_call: None,
            finish_reason: Some("stop".to_string()),
        });
    }
    if !line.starts_with("data: ") {
        return None;
    }
    let json_str = &line["data: ".len()..];
    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let choices = v.get("choices")?.as_array()?;
    let choice = choices.first()?;
    let delta = choice.get("delta")?;
    let finish_reason = choice
        .get("finish_reason")
        .and_then(|r| r.as_str())
        .map(|s| s.to_string());

    let text_delta = delta
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    // Parse partial tool call if present
    let tool_call = delta
        .get("tool_calls")
        .and_then(|tc| tc.as_array())
        .and_then(|arr| arr.first())
        .map(|tc| PartialToolCall {
            index: tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize,
            id: tc
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("")
                .to_string(),
            name: tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string(),
            arguments: tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
                .unwrap_or("")
                .to_string(),
        });

    let done = finish_reason.is_some();

    Some(StreamChunk {
        delta: text_delta,
        done,
        tool_call,
        finish_reason,
    })
}

/// Parse an Anthropic SSE stream line into a StreamChunk.
pub fn parse_anthropic_sse_line(line: &str) -> Option<StreamChunk> {
    let line = line.trim();
    if !line.starts_with("data: ") {
        return None;
    }
    let json_str = &line["data: ".len()..];
    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let event_type = v.get("type")?.as_str()?;

    match event_type {
        "content_block_delta" => {
            let delta = v.get("delta")?;
            let delta_type = delta.get("type")?.as_str()?;
            match delta_type {
                "text_delta" => {
                    let text = delta
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some(StreamChunk {
                        delta: text,
                        done: false,
                        tool_call: None,
                        finish_reason: None,
                    })
                }
                "input_json_delta" => {
                    let partial_json = delta
                        .get("partial_json")
                        .and_then(|p| p.as_str())
                        .unwrap_or("")
                        .to_string();
                    let index = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                    Some(StreamChunk {
                        delta: String::new(),
                        done: false,
                        tool_call: Some(PartialToolCall {
                            index,
                            id: String::new(),
                            name: String::new(),
                            arguments: partial_json,
                        }),
                        finish_reason: None,
                    })
                }
                _ => None,
            }
        }
        "message_stop" => Some(StreamChunk {
            delta: String::new(),
            done: true,
            tool_call: None,
            finish_reason: Some("stop".to_string()),
        }),
        "message_delta" => {
            let delta = v.get("delta")?;
            let stop_reason = delta
                .get("stop_reason")
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());
            if stop_reason.is_some() {
                Some(StreamChunk {
                    delta: String::new(),
                    done: true,
                    tool_call: None,
                    finish_reason: stop_reason,
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_openai_sse_text_chunk() {
        let line = r#"data: {"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}"#;
        let chunk = parse_openai_sse_line(line).unwrap();
        assert_eq!(chunk.delta, "Hello");
        assert!(!chunk.done);
    }

    #[test]
    fn test_parse_openai_sse_done() {
        let chunk = parse_openai_sse_line("data: [DONE]").unwrap();
        assert!(chunk.done);
    }

    #[test]
    fn test_parse_openai_sse_tool_call() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"shell","arguments":"{\"cmd\":"}}]},"finish_reason":null}]}"#;
        let chunk = parse_openai_sse_line(line).unwrap();
        assert!(chunk.tool_call.is_some());
        let tc = chunk.tool_call.unwrap();
        assert_eq!(tc.name, "shell");
        assert_eq!(tc.id, "call_abc");
    }

    #[test]
    fn test_parse_anthropic_text_delta() {
        let line = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#;
        let chunk = parse_anthropic_sse_line(line).unwrap();
        assert_eq!(chunk.delta, "Hi");
        assert!(!chunk.done);
    }

    #[test]
    fn test_parse_anthropic_stop() {
        let line = r#"data: {"type":"message_stop"}"#;
        let chunk = parse_anthropic_sse_line(line).unwrap();
        assert!(chunk.done);
    }

    #[tokio::test]
    async fn test_streaming_response_collect_text() {
        let (tx, rx) = mpsc::channel(10);
        let sr = StreamingResponse::new(rx);

        tx.send(StreamChunk {
            delta: "Hello ".to_string(),
            done: false,
            tool_call: None,
            finish_reason: None,
        })
        .await
        .unwrap();
        tx.send(StreamChunk {
            delta: "world".to_string(),
            done: true,
            tool_call: None,
            finish_reason: Some("stop".to_string()),
        })
        .await
        .unwrap();

        let text = sr.collect_text().await;
        assert_eq!(text, "Hello world");
    }
}
