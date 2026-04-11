//! Streaming tool call accumulation and response building.
//!
//! Provides utilities for assembling partial streaming tool call deltas into
//! complete [`ToolCall`] values, and for building a finished response from a
//! mixture of text content and tool call chunks.

use crate::providers::ToolCall;

// ── StreamingToolCall ────────────────────────────────────────────────────────

/// A single in-progress tool call being assembled from streaming deltas.
#[derive(Debug, Clone)]
pub struct StreamingToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl StreamingToolCall {
    fn new() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            arguments: String::new(),
        }
    }
}

// ── StreamingToolCallAccumulator ─────────────────────────────────────────────

/// Collects partial tool call deltas and assembles them into complete
/// [`ToolCall`] values once the stream finishes.
#[derive(Debug)]
pub struct StreamingToolCallAccumulator {
    pending: Vec<StreamingToolCall>,
}

impl StreamingToolCallAccumulator {
    /// Create an empty accumulator.
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    /// Append a partial tool call delta at the given `index`.
    ///
    /// If `index` refers to a position beyond the current length the
    /// accumulator is extended with empty entries so the index is valid.
    pub fn push_delta(
        &mut self,
        index: usize,
        id: Option<&str>,
        name: Option<&str>,
        arguments: Option<&str>,
    ) {
        // Grow the vec if necessary.
        while self.pending.len() <= index {
            self.pending.push(StreamingToolCall::new());
        }

        let entry = &mut self.pending[index];
        if let Some(id) = id {
            entry.id.push_str(id);
        }
        if let Some(name) = name {
            entry.name.push_str(name);
        }
        if let Some(args) = arguments {
            entry.arguments.push_str(args);
        }
    }

    /// Whether no deltas have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// The number of distinct tool calls tracked so far.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Convert all accumulated streaming calls into finalized [`ToolCall`]
    /// values, consuming the accumulator.
    pub fn finalize(self) -> Vec<ToolCall> {
        self.pending
            .into_iter()
            .filter(|tc| !tc.name.is_empty())
            .map(|tc| ToolCall {
                id: tc.id,
                name: tc.name,
                args: tc.arguments,
            })
            .collect()
    }

    /// Discard all accumulated state.
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

impl Default for StreamingToolCallAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

// ── StreamingResponse ────────────────────────────────────────────────────────

/// A fully assembled streaming response containing accumulated content and
/// finalized tool calls.
#[derive(Debug, Clone)]
pub struct StreamingResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finished: bool,
}

// ── StreamingResponseBuilder ─────────────────────────────────────────────────

/// Incrementally builds a [`StreamingResponse`] from a stream of text and
/// tool call chunks.
#[derive(Debug)]
pub struct StreamingResponseBuilder {
    content: String,
    tool_accumulator: StreamingToolCallAccumulator,
    finished: bool,
}

impl StreamingResponseBuilder {
    /// Create an empty builder.
    pub fn new() -> Self {
        Self {
            content: String::new(),
            tool_accumulator: StreamingToolCallAccumulator::new(),
            finished: false,
        }
    }

    /// Append a text chunk to the response content.
    pub fn push_content(&mut self, text: &str) {
        self.content.push_str(text);
    }

    /// Forward a partial tool call delta to the internal accumulator.
    pub fn push_tool_delta(
        &mut self,
        index: usize,
        id: Option<&str>,
        name: Option<&str>,
        arguments: Option<&str>,
    ) {
        self.tool_accumulator.push_delta(index, id, name, arguments);
    }

    /// Mark the stream as complete.
    pub fn set_finished(&mut self) {
        self.finished = true;
    }

    /// Consume the builder and produce the final [`StreamingResponse`].
    pub fn build(self) -> StreamingResponse {
        StreamingResponse {
            content: self.content,
            tool_calls: self.tool_accumulator.finalize(),
            finished: self.finished,
        }
    }
}

impl Default for StreamingResponseBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulator_single_tool_call() {
        let mut acc = StreamingToolCallAccumulator::new();
        acc.push_delta(0, Some("call_1"), Some("shell"), Some(r#"{"cmd":"ls"}"#));

        assert_eq!(acc.len(), 1);
        let calls = acc.finalize();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "shell");
        assert_eq!(calls[0].args, r#"{"cmd":"ls"}"#);
    }

    #[test]
    fn accumulator_multiple_tool_calls() {
        let mut acc = StreamingToolCallAccumulator::new();
        acc.push_delta(0, Some("call_1"), Some("shell"), Some(r#"{"cmd":"ls"}"#));
        acc.push_delta(1, Some("call_2"), Some("read_file"), Some(r#"{"path":"a.txt"}"#));

        assert_eq!(acc.len(), 2);
        let calls = acc.finalize();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(calls[1].name, "read_file");
    }

    #[test]
    fn accumulator_fragmented_arguments() {
        let mut acc = StreamingToolCallAccumulator::new();
        // First chunk: id + name + start of args
        acc.push_delta(0, Some("call_1"), Some("shell"), Some("{\"cm"));
        // Second chunk: more args
        acc.push_delta(0, None, None, Some("d\":\"ls\""));
        // Third chunk: close args
        acc.push_delta(0, None, None, Some("}"));

        assert_eq!(acc.len(), 1);
        let calls = acc.finalize();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "shell");
        assert_eq!(calls[0].args, r#"{"cmd":"ls"}"#);
    }

    #[test]
    fn accumulator_empty() {
        let acc = StreamingToolCallAccumulator::new();
        assert!(acc.is_empty());
        assert_eq!(acc.len(), 0);
        let calls = acc.finalize();
        assert!(calls.is_empty());
    }

    #[test]
    fn accumulator_clear() {
        let mut acc = StreamingToolCallAccumulator::new();
        acc.push_delta(0, Some("call_1"), Some("shell"), Some("{}"));
        assert!(!acc.is_empty());
        acc.clear();
        assert!(acc.is_empty());
    }

    #[test]
    fn accumulator_filters_nameless_entries() {
        let mut acc = StreamingToolCallAccumulator::new();
        // Entry at index 0 has no name — should be filtered out.
        acc.push_delta(0, Some("call_1"), None, Some("{}"));
        acc.push_delta(1, Some("call_2"), Some("shell"), Some("{}"));

        let calls = acc.finalize();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
    }

    #[test]
    fn builder_mixed_content_and_tool_calls() {
        let mut builder = StreamingResponseBuilder::new();
        builder.push_content("Hello ");
        builder.push_tool_delta(0, Some("call_1"), Some("shell"), Some("{\"cmd\":"));
        builder.push_content("world");
        builder.push_tool_delta(0, None, None, Some("\"ls\"}"));
        builder.set_finished();

        let response = builder.build();
        assert_eq!(response.content, "Hello world");
        assert!(response.finished);
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "shell");
        assert_eq!(response.tool_calls[0].args, r#"{"cmd":"ls"}"#);
    }

    #[test]
    fn builder_content_only() {
        let mut builder = StreamingResponseBuilder::new();
        builder.push_content("Just text");
        builder.set_finished();

        let response = builder.build();
        assert_eq!(response.content, "Just text");
        assert!(response.finished);
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn builder_not_finished() {
        let builder = StreamingResponseBuilder::new();
        let response = builder.build();
        assert!(!response.finished);
    }
}
