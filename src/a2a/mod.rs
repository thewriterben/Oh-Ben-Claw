//! Agent-to-Agent (A2A) protocol implementation — **v1.0 conformant subset**.
//!
//! Rewritten for Phase 15 WS3 against the A2A v1.0 specification
//! (<https://a2a-protocol.org/latest/specification/>, Linux Foundation).
//! The previous implementation predated the stable spec and matched neither
//! v0.3.0 nor v1.0 on the wire.
//!
//! ## Supported subset
//!
//! - **Binding:** JSON-RPC 2.0 over HTTP (`protocolBinding: "JSONRPC"`).
//!   gRPC and HTTP+JSON bindings are not implemented.
//! - **Operations:** `SendMessage`, `GetTask`, `CancelTask` (PascalCase per
//!   v1.0). Streaming (`SendStreamingMessage`/`SubscribeToTask`), `ListTasks`,
//!   push-notification configs, and the extended agent card return
//!   `UnsupportedOperationError` (-32004).
//! - **Discovery:** agent card served at `/.well-known/agent-card.json`.
//! - **Versioning:** the `A2A-Version` header is sent by the client and
//!   validated by the server; per spec, an absent header means `0.3`, which
//!   this implementation does not speak → `VersionNotSupportedError` (-32009).
//!
//! ## v1.0 conventions honoured here
//!
//! - JSON field names are camelCase (ProtoJSON).
//! - Enum values are SCREAMING_SNAKE_CASE proto names (`TASK_STATE_*`,
//!   `ROLE_*`) — v0.3.0's kebab-case forms are gone.
//! - `Part` has no `kind` discriminator: content is a oneof discriminated by
//!   member presence (`text` | `raw` | `url` | `data`), plus optional
//!   `mediaType` / `filename` / `metadata`.
//! - JSON-RPC errors carry a `google.rpc.ErrorInfo` in `error.data` with
//!   `domain: "a2a-protocol.org"` and an UPPER_SNAKE_CASE `reason`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Protocol version spoken by this implementation (Major.Minor only).
pub const A2A_PROTOCOL_VERSION: &str = "1.0";
/// Well-known discovery path (v1.0; the pre-0.3 `agent.json` is legacy).
pub const WELL_KNOWN_AGENT_CARD_PATH: &str = "/.well-known/agent-card.json";
/// The JSON-RPC binding identifier used in `AgentInterface.protocolBinding`.
pub const PROTOCOL_BINDING_JSONRPC: &str = "JSONRPC";

// ── Agent Card ────────────────────────────────────────────────────────────────

/// Describes an agent's capabilities, served at `/.well-known/agent-card.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    pub name: String,
    pub description: String,
    /// The agent's own version (not the protocol version).
    pub version: String,
    /// Ordered list of supported interfaces; the first entry is preferred.
    pub supported_interfaces: Vec<AgentInterface>,
    pub capabilities: AgentCapabilities,
    /// Default accepted input media types.
    #[serde(default)]
    pub default_input_modes: Vec<String>,
    /// Default produced output media types.
    #[serde(default)]
    pub default_output_modes: Vec<String>,
    #[serde(default)]
    pub skills: Vec<AgentSkill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
}

/// One transport interface an agent exposes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentInterface {
    pub url: String,
    /// Core values: `"JSONRPC"`, `"GRPC"`, `"HTTP+JSON"`.
    pub protocol_binding: String,
    /// Major.Minor protocol version, e.g. `"1.0"`.
    pub protocol_version: String,
    /// Opaque routing id; when set, clients must echo it in requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
}

/// Optional capability flags (all default false in this subset).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub push_notifications: bool,
    #[serde(default)]
    pub extended_agent_card: bool,
}

/// The organization behind an agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentProvider {
    pub url: String,
    pub organization: String,
}

/// A skill that an agent can perform (v1.0: `id` and `tags` are required).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_modes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_modes: Option<Vec<String>>,
}

// ── Task / Message / Part / Artifact ──────────────────────────────────────────

/// Task lifecycle states. JSON values are the proto enum names.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskState {
    #[serde(rename = "TASK_STATE_UNSPECIFIED")]
    Unspecified,
    #[serde(rename = "TASK_STATE_SUBMITTED")]
    Submitted,
    #[serde(rename = "TASK_STATE_WORKING")]
    Working,
    #[serde(rename = "TASK_STATE_COMPLETED")]
    Completed,
    #[serde(rename = "TASK_STATE_FAILED")]
    Failed,
    #[serde(rename = "TASK_STATE_CANCELED")]
    Canceled,
    #[serde(rename = "TASK_STATE_INPUT_REQUIRED")]
    InputRequired,
    #[serde(rename = "TASK_STATE_REJECTED")]
    Rejected,
    #[serde(rename = "TASK_STATE_AUTH_REQUIRED")]
    AuthRequired,
}

impl TaskState {
    /// Terminal states cannot transition further (and cannot be canceled).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Canceled | Self::Rejected
        )
    }
}

/// Status of a task at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskStatus {
    pub state: TaskState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    /// ISO 8601 UTC, e.g. `2026-06-05T10:00:00Z`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// A unit of work. `id` is server-generated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

/// Message direction. JSON values are the proto enum names.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Role {
    #[serde(rename = "ROLE_UNSPECIFIED")]
    Unspecified,
    #[serde(rename = "ROLE_USER")]
    User,
    #[serde(rename = "ROLE_AGENT")]
    Agent,
}

/// One content part. v1.0 removed the `kind` discriminator: exactly one of
/// `text` / `raw` / `url` / `data` should be present (member-presence oneof).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Inline bytes (base64 in JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// Pointer to file content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Structured data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// Replaces v0.3.0 `mimeType`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl Part {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Default::default()
        }
    }

    pub fn data(data: Value) -> Self {
        Self {
            data: Some(data),
            ..Default::default()
        }
    }

    /// True when exactly one content member is present.
    pub fn is_valid_oneof(&self) -> bool {
        [
            self.text.is_some(),
            self.raw.is_some(),
            self.url.is_some(),
            self.data.is_some(),
        ]
        .iter()
        .filter(|present| **present)
        .count()
            == 1
    }
}

/// A message exchanged between client and agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub message_id: String,
    pub role: Role,
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl Message {
    /// Build a user message with a single text part and a fresh messageId.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            message_id: uuid::Uuid::new_v4().to_string(),
            role: Role::User,
            parts: vec![Part::text(text)],
            task_id: None,
            context_id: None,
            metadata: None,
        }
    }
}

/// An output artifact produced by a task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub artifact_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Must contain at least one part.
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

/// Result of `SendMessage`: the server returns either a task or a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SendMessageResult {
    #[serde(rename = "task")]
    Task(Task),
    #[serde(rename = "message")]
    Message(Message),
}

// ── A2A Error Codes ───────────────────────────────────────────────────────────

/// A2A-specific JSON-RPC error codes (spec §5.4).
pub mod error_codes {
    pub const TASK_NOT_FOUND: i64 = -32001;
    pub const TASK_NOT_CANCELABLE: i64 = -32002;
    pub const PUSH_NOTIFICATION_NOT_SUPPORTED: i64 = -32003;
    pub const UNSUPPORTED_OPERATION: i64 = -32004;
    pub const CONTENT_TYPE_NOT_SUPPORTED: i64 = -32005;
    pub const INVALID_AGENT_RESPONSE: i64 = -32006;
    pub const EXTENDED_AGENT_CARD_NOT_CONFIGURED: i64 = -32007;
    pub const EXTENSION_SUPPORT_REQUIRED: i64 = -32008;
    pub const VERSION_NOT_SUPPORTED: i64 = -32009;
}

/// Build the spec-required `google.rpc.ErrorInfo` payload for `error.data`.
///
/// `reason` is the error type in UPPER_SNAKE_CASE without the "Error" suffix,
/// e.g. `TASK_NOT_FOUND`.
pub fn error_info(reason: &str) -> Value {
    json!({
        "@type": "type.googleapis.com/google.rpc.ErrorInfo",
        "reason": reason,
        "domain": "a2a-protocol.org"
    })
}

/// JSON-RPC 2.0 message envelope for A2A communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2AMessage {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
}

impl A2AMessage {
    /// Create a new JSON-RPC 2.0 message.
    pub fn new(method: &str, params: Option<Value>, id: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
            id,
        }
    }
}

// ── A2A Client (JSONRPC binding) ──────────────────────────────────────────────

/// Client for the JSON-RPC binding of A2A v1.0.
pub struct A2AClient {
    base_url: String,
    client: reqwest::Client,
    next_id: u64,
}

impl A2AClient {
    /// Create a new A2A client pointing at the given agent URL.
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: reqwest::Client::new(),
            next_id: 1,
        }
    }

    /// Discover the remote agent's capabilities via
    /// `/.well-known/agent-card.json`.
    pub async fn discover(&self) -> Result<AgentCard> {
        let url = format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            WELL_KNOWN_AGENT_CARD_PATH
        );
        let card: AgentCard = self.client.get(&url).send().await?.json().await?;
        Ok(card)
    }

    /// Send a message (`SendMessage`); blocking by default per spec.
    pub async fn send_message(&mut self, message: Message) -> Result<SendMessageResult> {
        let result = self
            .rpc("SendMessage", json!({ "message": message }))
            .await?;
        Ok(serde_json::from_value(result)?)
    }

    /// Fetch a task by id (`GetTask`).
    pub async fn get_task(&mut self, task_id: &str) -> Result<Task> {
        let result = self.rpc("GetTask", json!({ "id": task_id })).await?;
        Ok(serde_json::from_value(result)?)
    }

    /// Request cancellation of a task (`CancelTask`); returns the updated task.
    pub async fn cancel_task(&mut self, task_id: &str) -> Result<Task> {
        let result = self.rpc("CancelTask", json!({ "id": task_id })).await?;
        Ok(serde_json::from_value(result)?)
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = A2AMessage::new(method, Some(params), Some(json!(id)));
        let resp: Value = self
            .client
            .post(&self.base_url)
            .header("A2A-Version", A2A_PROTOCOL_VERSION)
            .json(&req)
            .send()
            .await?
            .json()
            .await?;
        if let Some(err) = resp.get("error") {
            anyhow::bail!(
                "A2A error {}: {}",
                err["code"].as_i64().unwrap_or(0),
                err["message"].as_str().unwrap_or("unknown")
            );
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }
}

// ── A2A Server (JSONRPC binding) ──────────────────────────────────────────────

/// Exposes a local agent as an A2A v1.0 endpoint with an in-memory task store.
///
/// `SendMessage` runs the (stub) skill synchronously and records a completed
/// task; real deployments replace [`A2AServer::execute`] with agent dispatch.
pub struct A2AServer {
    card: AgentCard,
    tasks: HashMap<String, Task>,
}

impl A2AServer {
    /// Create a new A2A server with the given agent card.
    pub fn new(card: AgentCard) -> Self {
        Self {
            card,
            tasks: HashMap::new(),
        }
    }

    /// Build a minimal v1.0 agent card for this Oh-Ben-Claw instance.
    pub fn build_card(
        name: &str,
        description: &str,
        url: &str,
        agent_version: &str,
        skill_names: &[String],
    ) -> AgentCard {
        AgentCard {
            name: name.to_string(),
            description: description.to_string(),
            version: agent_version.to_string(),
            supported_interfaces: vec![AgentInterface {
                url: url.to_string(),
                protocol_binding: PROTOCOL_BINDING_JSONRPC.to_string(),
                protocol_version: A2A_PROTOCOL_VERSION.to_string(),
                tenant: None,
            }],
            capabilities: AgentCapabilities::default(),
            default_input_modes: vec!["text/plain".to_string(), "application/json".to_string()],
            default_output_modes: vec!["text/plain".to_string(), "application/json".to_string()],
            skills: skill_names
                .iter()
                .map(|s| AgentSkill {
                    id: s.clone(),
                    name: s.clone(),
                    description: format!("Oh-Ben-Claw skill: {s}"),
                    tags: vec!["oh-ben-claw".to_string()],
                    examples: Vec::new(),
                    input_modes: None,
                    output_modes: None,
                })
                .collect(),
            provider: None,
            documentation_url: None,
            icon_url: None,
        }
    }

    /// Return a reference to this server's agent card.
    pub fn agent_card(&self) -> &AgentCard {
        &self.card
    }

    /// Handle a discovery request by returning the agent card.
    pub fn handle_discover(&self) -> AgentCard {
        self.card.clone()
    }

    /// Validate the `A2A-Version` service parameter.
    ///
    /// Per spec an absent header is interpreted as `0.3`, which this
    /// implementation does not speak.
    pub fn validate_version(version_header: Option<&str>) -> Result<(), Value> {
        let effective = version_header.unwrap_or("0.3");
        if effective == A2A_PROTOCOL_VERSION {
            Ok(())
        } else {
            Err(json!({
                "code": error_codes::VERSION_NOT_SUPPORTED,
                "message": format!(
                    "A2A version '{effective}' is not supported; this agent speaks {A2A_PROTOCOL_VERSION}"
                ),
                "data": error_info("VERSION_NOT_SUPPORTED"),
            }))
        }
    }

    /// Handle one JSON-RPC request (after version validation).
    pub fn handle_jsonrpc(&mut self, req: &A2AMessage) -> Value {
        let id = req.id.clone().unwrap_or(Value::Null);
        let params = req.params.clone().unwrap_or(json!({}));

        let result: Result<Value, Value> = match req.method.as_str() {
            "SendMessage" => self.handle_send_message(&params),
            "GetTask" => self.handle_get_task(&params),
            "CancelTask" => self.handle_cancel_task(&params),
            "ListTasks" | "SendStreamingMessage" | "SubscribeToTask" | "GetExtendedAgentCard"
            | "CreateTaskPushNotificationConfig" | "GetTaskPushNotificationConfig"
            | "ListTaskPushNotificationConfigs" | "DeleteTaskPushNotificationConfig" => {
                Err(json!({
                    "code": error_codes::UNSUPPORTED_OPERATION,
                    "message": format!("operation '{}' is not supported by this agent", req.method),
                    "data": error_info("UNSUPPORTED_OPERATION"),
                }))
            }
            other => Err(json!({
                "code": -32601,
                "message": format!("Method not found: {other}"),
            })),
        };

        match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
        }
    }

    fn handle_send_message(&mut self, params: &Value) -> Result<Value, Value> {
        let message: Message = serde_json::from_value(params["message"].clone()).map_err(|e| {
            json!({"code": -32602, "message": format!("invalid SendMessage params: {e}")})
        })?;

        if !message.parts.iter().all(Part::is_valid_oneof) {
            return Err(json!({
                "code": -32602,
                "message": "each part must contain exactly one of text/raw/url/data",
            }));
        }

        let task = self.execute(message);
        self.tasks.insert(task.id.clone(), task.clone());
        Ok(json!({ "task": task }))
    }

    fn handle_get_task(&mut self, params: &Value) -> Result<Value, Value> {
        let id = params["id"].as_str().unwrap_or_default();
        match self.tasks.get(id) {
            Some(task) => Ok(serde_json::to_value(task).unwrap_or(Value::Null)),
            None => Err(json!({
                "code": error_codes::TASK_NOT_FOUND,
                "message": format!("task '{id}' not found"),
                "data": error_info("TASK_NOT_FOUND"),
            })),
        }
    }

    fn handle_cancel_task(&mut self, params: &Value) -> Result<Value, Value> {
        let id = params["id"].as_str().unwrap_or_default().to_string();
        let task = match self.tasks.get_mut(&id) {
            Some(t) => t,
            None => {
                return Err(json!({
                    "code": error_codes::TASK_NOT_FOUND,
                    "message": format!("task '{id}' not found"),
                    "data": error_info("TASK_NOT_FOUND"),
                }))
            }
        };
        if task.status.state.is_terminal() {
            return Err(json!({
                "code": error_codes::TASK_NOT_CANCELABLE,
                "message": format!("task '{id}' is in a terminal state and cannot be canceled"),
                "data": error_info("TASK_NOT_CANCELABLE"),
            }));
        }
        task.status.state = TaskState::Canceled;
        Ok(serde_json::to_value(&*task).unwrap_or(Value::Null))
    }

    /// Stub execution: records the inbound message in history and completes.
    ///
    /// Real deployments dispatch to the agent loop here.
    fn execute(&self, message: Message) -> Task {
        let context_id = message
            .context_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        Task {
            id: uuid::Uuid::new_v4().to_string(),
            context_id: Some(context_id),
            status: TaskStatus {
                state: TaskState::Completed,
                message: None,
                timestamp: None,
            },
            artifacts: Vec::new(),
            history: vec![message],
            metadata: None,
        }
    }

    /// Number of tasks held in the in-memory store (for tests/monitoring).
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_card() -> AgentCard {
        A2AServer::build_card(
            "test-agent",
            "A test agent",
            "http://localhost:8080/a2a",
            "1.2.0",
            &["summarize".to_string()],
        )
    }

    fn server() -> A2AServer {
        A2AServer::new(sample_card())
    }

    fn send(server: &mut A2AServer, text: &str) -> Task {
        let msg = Message::user_text(text);
        let resp = server.handle_jsonrpc(&A2AMessage::new(
            "SendMessage",
            Some(json!({ "message": msg })),
            Some(json!(1)),
        ));
        serde_json::from_value(resp["result"]["task"].clone()).expect("task in result")
    }

    // ── Agent card conformance ────────────────────────────────────────────

    #[test]
    fn agent_card_v1_shape() {
        let card = sample_card();
        let v = serde_json::to_value(&card).unwrap();
        // camelCase field names; supportedInterfaces replaces url/preferredTransport.
        assert!(v["supportedInterfaces"].is_array());
        assert_eq!(v["supportedInterfaces"][0]["protocolBinding"], "JSONRPC");
        assert_eq!(v["supportedInterfaces"][0]["protocolVersion"], "1.0");
        assert!(v.get("url").is_none(), "top-level url was removed in v1.0");
        assert!(
            v.get("protocolVersion").is_none(),
            "protocolVersion moved to supportedInterfaces in v1.0"
        );
        assert!(v["capabilities"].is_object());
        assert_eq!(v["defaultInputModes"][0], "text/plain");
        // Skills require id + tags in v1.0.
        assert_eq!(v["skills"][0]["id"], "summarize");
        assert!(v["skills"][0]["tags"].is_array());
    }

    #[test]
    fn agent_card_round_trip() {
        let card = sample_card();
        let json = serde_json::to_string(&card).unwrap();
        let back: AgentCard = serde_json::from_str(&json).unwrap();
        assert_eq!(card, back);
    }

    #[test]
    fn well_known_path_is_agent_card_json() {
        assert_eq!(WELL_KNOWN_AGENT_CARD_PATH, "/.well-known/agent-card.json");
    }

    // ── Enum wire formats ─────────────────────────────────────────────────

    #[test]
    fn task_state_serializes_to_proto_names() {
        let pairs = [
            (TaskState::Submitted, "\"TASK_STATE_SUBMITTED\""),
            (TaskState::Working, "\"TASK_STATE_WORKING\""),
            (TaskState::Completed, "\"TASK_STATE_COMPLETED\""),
            (TaskState::Failed, "\"TASK_STATE_FAILED\""),
            (TaskState::Canceled, "\"TASK_STATE_CANCELED\""),
            (TaskState::InputRequired, "\"TASK_STATE_INPUT_REQUIRED\""),
            (TaskState::Rejected, "\"TASK_STATE_REJECTED\""),
            (TaskState::AuthRequired, "\"TASK_STATE_AUTH_REQUIRED\""),
        ];
        for (state, expected) in pairs {
            assert_eq!(serde_json::to_string(&state).unwrap(), expected);
        }
    }

    #[test]
    fn role_serializes_to_proto_names() {
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"ROLE_USER\"");
        assert_eq!(
            serde_json::to_string(&Role::Agent).unwrap(),
            "\"ROLE_AGENT\""
        );
    }

    #[test]
    fn terminal_states() {
        assert!(TaskState::Completed.is_terminal());
        assert!(TaskState::Canceled.is_terminal());
        assert!(TaskState::Rejected.is_terminal());
        assert!(!TaskState::Working.is_terminal());
        assert!(!TaskState::InputRequired.is_terminal());
    }

    // ── Part oneof (no kind discriminator) ────────────────────────────────

    #[test]
    fn part_has_no_kind_discriminator() {
        let part = Part::text("hello");
        let v = serde_json::to_value(&part).unwrap();
        assert!(v.get("kind").is_none());
        assert_eq!(v["text"], "hello");
        // Absent oneof members are omitted entirely.
        assert!(v.get("raw").is_none());
        assert!(v.get("data").is_none());
    }

    #[test]
    fn part_oneof_validation() {
        assert!(Part::text("x").is_valid_oneof());
        assert!(Part::data(json!({"k": 1})).is_valid_oneof());
        let empty = Part::default();
        assert!(!empty.is_valid_oneof());
        let both = Part {
            text: Some("x".into()),
            data: Some(json!(1)),
            ..Default::default()
        };
        assert!(!both.is_valid_oneof());
    }

    #[test]
    fn part_media_type_replaces_mime_type() {
        let part = Part {
            raw: Some("aGVsbG8=".into()),
            media_type: Some("image/jpeg".into()),
            filename: Some("photo.jpg".into()),
            ..Default::default()
        };
        let v = serde_json::to_value(&part).unwrap();
        assert_eq!(v["mediaType"], "image/jpeg");
        assert!(v.get("mimeType").is_none());
    }

    // ── Server lifecycle ──────────────────────────────────────────────────

    #[test]
    fn send_message_creates_completed_task() {
        let mut s = server();
        let task = send(&mut s, "do the thing");
        assert_eq!(task.status.state, TaskState::Completed);
        assert!(task.context_id.is_some());
        assert_eq!(task.history.len(), 1);
        assert_eq!(s.task_count(), 1);
    }

    #[test]
    fn get_task_returns_stored_task() {
        let mut s = server();
        let task = send(&mut s, "hello");
        let resp = s.handle_jsonrpc(&A2AMessage::new(
            "GetTask",
            Some(json!({"id": task.id})),
            Some(json!(2)),
        ));
        let fetched: Task = serde_json::from_value(resp["result"].clone()).unwrap();
        assert_eq!(fetched.id, task.id);
    }

    #[test]
    fn get_unknown_task_returns_task_not_found() {
        let mut s = server();
        let resp = s.handle_jsonrpc(&A2AMessage::new(
            "GetTask",
            Some(json!({"id": "nope"})),
            Some(json!(3)),
        ));
        assert_eq!(resp["error"]["code"], error_codes::TASK_NOT_FOUND);
        assert_eq!(resp["error"]["data"]["reason"], "TASK_NOT_FOUND");
        assert_eq!(resp["error"]["data"]["domain"], "a2a-protocol.org");
    }

    #[test]
    fn cancel_terminal_task_returns_not_cancelable() {
        let mut s = server();
        let task = send(&mut s, "hello"); // stub completes immediately → terminal
        let resp = s.handle_jsonrpc(&A2AMessage::new(
            "CancelTask",
            Some(json!({"id": task.id})),
            Some(json!(4)),
        ));
        assert_eq!(resp["error"]["code"], error_codes::TASK_NOT_CANCELABLE);
    }

    #[test]
    fn unsupported_operations_return_32004() {
        let mut s = server();
        for method in ["ListTasks", "SendStreamingMessage", "GetExtendedAgentCard"] {
            let resp = s.handle_jsonrpc(&A2AMessage::new(method, Some(json!({})), Some(json!(5))));
            assert_eq!(
                resp["error"]["code"],
                error_codes::UNSUPPORTED_OPERATION,
                "method {method}"
            );
        }
    }

    #[test]
    fn invalid_part_oneof_rejected() {
        let mut s = server();
        let msg = json!({
            "messageId": "m1",
            "role": "ROLE_USER",
            "parts": [{"text": "x", "data": {"y": 1}}],
        });
        let resp = s.handle_jsonrpc(&A2AMessage::new(
            "SendMessage",
            Some(json!({"message": msg})),
            Some(json!(6)),
        ));
        assert_eq!(resp["error"]["code"], -32602);
    }

    // ── Version validation ────────────────────────────────────────────────

    #[test]
    fn version_header_1_0_accepted() {
        assert!(A2AServer::validate_version(Some("1.0")).is_ok());
    }

    #[test]
    fn absent_version_header_means_0_3_and_is_rejected() {
        let err = A2AServer::validate_version(None).unwrap_err();
        assert_eq!(err["code"], error_codes::VERSION_NOT_SUPPORTED);
    }

    #[test]
    fn unknown_version_rejected() {
        let err = A2AServer::validate_version(Some("0.3")).unwrap_err();
        assert_eq!(err["code"], error_codes::VERSION_NOT_SUPPORTED);
        assert_eq!(err["data"]["reason"], "VERSION_NOT_SUPPORTED");
    }
}
