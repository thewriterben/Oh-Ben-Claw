//! The executor that makes `a2a` do something: dispatch to the real agent.
//!
//! [`crate::a2a`] defines the protocol, the task store and the
//! [`TaskExecutor`](crate::a2a::TaskExecutor) trait, and names nothing else in
//! this crate. That is the property that makes it separable into a crate, and
//! it is worth keeping: so the trait is declared there and implemented *here*,
//! next to [`Agent`], rather than the other way around.
//!
//! Before 2026-08-08 the only implementation was the echo stub, and there was
//! no transport to reach even that. `SendMessage` returned the caller's own
//! message back as a completed task with no artifacts, which is a conformant
//! response and not an agent.

use std::sync::Arc;

use async_trait::async_trait;

use crate::a2a::{Artifact, Message, Part, Role, Task, TaskExecutor, TaskState, TaskStatus};
use crate::agent::Agent;
use crate::config::ProviderConfig;

/// Runs an inbound A2A message through [`Agent::process`] and returns the
/// reply as a completed task with one text artifact.
pub struct AgentExecutor {
    agent: Arc<Agent>,
    provider: ProviderConfig,
}

impl AgentExecutor {
    /// Wrap an agent so it can answer A2A `SendMessage` calls.
    pub fn new(agent: Arc<Agent>, provider: ProviderConfig) -> Self {
        Self { agent, provider }
    }

    /// The text an inbound message carries, concatenated across its text parts.
    ///
    /// A2A `Part` is a oneof — text, raw, url or data. Only text is dispatched;
    /// the others are carried in history but not interpreted, which is a limit
    /// worth stating rather than a behaviour worth implying.
    fn prompt_of(message: &Message) -> String {
        message
            .parts
            .iter()
            .filter_map(|p| p.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[async_trait]
impl TaskExecutor for AgentExecutor {
    async fn execute(&self, message: Message) -> Task {
        // The A2A context id is the conversation identity, so it is the session
        // key: two messages in one context share the agent's history, which is
        // what a caller sending a follow-up expects.
        let context_id = message
            .context_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let prompt = Self::prompt_of(&message);

        if prompt.trim().is_empty() {
            return failed_task(
                context_id,
                message,
                "no text part in message; this agent dispatches text only",
            );
        }

        match self
            .agent
            .process(&context_id, &prompt, &self.provider)
            .await
        {
            Ok(response) => {
                let reply = Message {
                    role: Role::Agent,
                    parts: vec![Part::text(response.message.clone())],
                    message_id: uuid::Uuid::new_v4().to_string(),
                    context_id: Some(context_id.clone()),
                    task_id: None,
                    metadata: None,
                };
                Task {
                    id: uuid::Uuid::new_v4().to_string(),
                    context_id: Some(context_id),
                    status: TaskStatus {
                        state: TaskState::Completed,
                        message: Some(reply.clone()),
                        timestamp: None,
                    },
                    artifacts: vec![Artifact {
                        artifact_id: uuid::Uuid::new_v4().to_string(),
                        name: Some("response".to_string()),
                        description: None,
                        parts: vec![Part::text(response.message)],
                        metadata: None,
                    }],
                    history: vec![message, reply],
                    metadata: None,
                }
            }
            // A model or tool failure is a *failed task*, not a JSON-RPC error:
            // the request was well formed and the protocol worked. Conflating
            // the two is how a caller ends up retrying a prompt that will never
            // succeed.
            Err(e) => failed_task(context_id, message, &format!("agent failed: {e}")),
        }
    }
}

fn failed_task(context_id: String, message: Message, why: &str) -> Task {
    let note = Message {
        role: Role::Agent,
        parts: vec![Part::text(why.to_string())],
        message_id: uuid::Uuid::new_v4().to_string(),
        context_id: Some(context_id.clone()),
        task_id: None,
        metadata: None,
    };
    Task {
        id: uuid::Uuid::new_v4().to_string(),
        context_id: Some(context_id),
        status: TaskStatus {
            state: TaskState::Failed,
            message: Some(note.clone()),
            timestamp: None,
        },
        artifacts: Vec::new(),
        history: vec![message, note],
        metadata: None,
    }
}
