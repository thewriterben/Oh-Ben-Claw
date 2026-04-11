//! Agent-to-Agent (A2A) protocol implementation.
//!
//! The A2A protocol (https://google.github.io/A2A/) enables interoperability
//! between AI agents across different platforms. This module provides the
//! core types and a lightweight client/server implementation.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Core A2A Types ────────────────────────────────────────────────────────────

/// Describes an agent's capabilities, served at `/.well-known/agent.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentCard {
    pub name: String,
    pub description: String,
    pub url: String,
    #[serde(default)]
    pub skills: Vec<A2ASkill>,
    #[serde(default)]
    pub supported_input_modes: Vec<String>,
    #[serde(default)]
    pub supported_output_modes: Vec<String>,
}

/// A skill that an agent can perform.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct A2ASkill {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

/// A request to perform a task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskRequest {
    pub id: String,
    pub skill: String,
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

/// Response to a task request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskResponse {
    pub id: String,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

/// Status of a task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

/// An output artifact produced by a task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    pub name: String,
    pub mime_type: String,
    pub data: String,
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

// ── A2A Client ────────────────────────────────────────────────────────────────

/// Client for sending task requests to remote A2A agents.
pub struct A2AClient {
    base_url: String,
    client: reqwest::Client,
}

impl A2AClient {
    /// Create a new A2A client pointing at the given agent URL.
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: reqwest::Client::new(),
        }
    }

    /// Discover the remote agent's capabilities via `/.well-known/agent.json`.
    pub async fn discover(&self) -> Result<AgentCard> {
        let url = format!(
            "{}/.well-known/agent.json",
            self.base_url.trim_end_matches('/')
        );
        let card: AgentCard = self.client.get(&url).send().await?.json().await?;
        Ok(card)
    }

    /// Send a task request to the remote agent.
    pub async fn send_task(&self, request: TaskRequest) -> Result<TaskResponse> {
        let url = format!("{}/tasks", self.base_url.trim_end_matches('/'));
        let response: TaskResponse = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await?
            .json()
            .await?;
        Ok(response)
    }

    /// Check the status of a previously submitted task.
    pub async fn get_task_status(&self, task_id: &str) -> Result<TaskStatus> {
        let url = format!(
            "{}/tasks/{}/status",
            self.base_url.trim_end_matches('/'),
            task_id
        );
        let status: TaskStatus = self.client.get(&url).send().await?.json().await?;
        Ok(status)
    }
}

// ── A2A Server ────────────────────────────────────────────────────────────────

/// Exposes a local agent as an A2A endpoint.
pub struct A2AServer {
    card: AgentCard,
}

impl A2AServer {
    /// Create a new A2A server with the given agent card.
    pub fn new(card: AgentCard) -> Self {
        Self { card }
    }

    /// Return a reference to this server's agent card.
    pub fn agent_card(&self) -> &AgentCard {
        &self.card
    }

    /// Handle a discovery request by returning the agent card.
    pub fn handle_discover(&self) -> AgentCard {
        self.card.clone()
    }

    /// Handle a task request and return a stub response.
    ///
    /// Real implementations should dispatch to the appropriate skill handler;
    /// this stub immediately returns a `Completed` response with no output.
    pub fn handle_task(&self, request: &TaskRequest) -> TaskResponse {
        TaskResponse {
            id: request.id.clone(),
            status: TaskStatus::Completed,
            output: None,
            artifacts: vec![],
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_card() -> AgentCard {
        AgentCard {
            name: "test-agent".to_string(),
            description: "A test agent".to_string(),
            url: "http://localhost:8080".to_string(),
            skills: vec![A2ASkill {
                name: "summarize".to_string(),
                description: "Summarize text".to_string(),
                input_schema: Some(json!({"type": "string"})),
                output_schema: Some(json!({"type": "string"})),
            }],
            supported_input_modes: vec!["text".to_string()],
            supported_output_modes: vec!["text".to_string()],
        }
    }

    fn sample_task_request() -> TaskRequest {
        TaskRequest {
            id: "task-001".to_string(),
            skill: "summarize".to_string(),
            input: json!({"text": "Hello, world!"}),
            metadata: Some(json!({"priority": "high"})),
        }
    }

    #[test]
    fn agent_card_serialization_round_trip() {
        let card = sample_card();
        let json = serde_json::to_string(&card).expect("serialize");
        let deserialized: AgentCard = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(card, deserialized);
    }

    #[test]
    fn task_request_response_round_trip() {
        let request = sample_task_request();
        let json_req = serde_json::to_string(&request).expect("serialize request");
        let deser_req: TaskRequest = serde_json::from_str(&json_req).expect("deserialize request");
        assert_eq!(request, deser_req);

        let response = TaskResponse {
            id: "task-001".to_string(),
            status: TaskStatus::Completed,
            output: Some(json!({"summary": "Hi!"})),
            artifacts: vec![Artifact {
                name: "result.txt".to_string(),
                mime_type: "text/plain".to_string(),
                data: "Hi!".to_string(),
            }],
        };
        let json_resp = serde_json::to_string(&response).expect("serialize response");
        let deser_resp: TaskResponse =
            serde_json::from_str(&json_resp).expect("deserialize response");
        assert_eq!(response, deser_resp);
    }

    #[test]
    fn a2a_client_construction() {
        let client = A2AClient::new("http://localhost:9000".to_string());
        assert_eq!(client.base_url, "http://localhost:9000");
    }

    #[test]
    fn a2a_server_handle_discover() {
        let card = sample_card();
        let server = A2AServer::new(card.clone());
        assert_eq!(server.agent_card(), &card);
        assert_eq!(server.handle_discover(), card);
    }

    #[test]
    fn a2a_server_handle_task() {
        let server = A2AServer::new(sample_card());
        let request = sample_task_request();
        let response = server.handle_task(&request);
        assert_eq!(response.id, request.id);
        assert_eq!(response.status, TaskStatus::Completed);
        assert!(response.artifacts.is_empty());
    }

    #[test]
    fn task_status_variants_serialize() {
        let variants = [
            (TaskStatus::Pending, "\"pending\""),
            (TaskStatus::InProgress, "\"in_progress\""),
            (TaskStatus::Completed, "\"completed\""),
            (TaskStatus::Failed, "\"failed\""),
            (TaskStatus::Cancelled, "\"cancelled\""),
        ];
        for (status, expected) in &variants {
            let json = serde_json::to_string(status).expect("serialize status");
            assert_eq!(&json, expected);
            let deser: TaskStatus = serde_json::from_str(expected).expect("deserialize status");
            assert_eq!(status, &deser);
        }
    }
}
