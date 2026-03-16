//! Hook handler trait for lifecycle events.

use serde_json::Value;

/// The result returned by a modifying hook.
///
/// Hooks that can veto an operation (e.g., `on_message_received`) return this type.
#[derive(Debug, Clone)]
pub enum HookResult {
    /// Allow the operation to proceed.
    Continue,
    /// Cancel the operation with a human-readable reason.
    Cancel { reason: String },
}

/// A lifecycle hook handler.
///
/// Implement this trait to react to agent events. All methods have default
/// no-op implementations so you only override what you need.
///
/// Hooks that can cancel an operation return [`HookResult`]; pure observers
/// return `()`.
pub trait HookHandler: Send + Sync {
    /// The execution priority of this handler.
    ///
    /// Higher values run before lower values. Handlers with the same priority
    /// run in registration order.
    fn priority(&self) -> i32 {
        0
    }

    /// Called when a new session starts.
    fn on_session_start(&self, _session_id: &str, _channel: &str) {}

    /// Called when a session ends.
    fn on_session_end(&self, _session_id: &str) {}

    /// Called when a message is received from the user.
    ///
    /// Return [`HookResult::Cancel`] to suppress the message.
    fn on_message_received(&self, _session_id: &str, _content: &str) -> HookResult {
        HookResult::Continue
    }

    /// Called before a tool is invoked.
    ///
    /// Return [`HookResult::Cancel`] to prevent the tool from running.
    fn on_tool_call(&self, _tool_name: &str, _args: &Value) -> HookResult {
        HookResult::Continue
    }

    /// Called after a tool returns its result.
    fn on_tool_result(&self, _tool_name: &str, _result: &str) {}

    /// Called when the agent produces a response.
    fn on_agent_response(&self, _session_id: &str, _response: &str) {}

    /// Called when the agent process starts.
    fn on_agent_start(&self) {}

    /// Called when the agent process stops.
    fn on_agent_stop(&self) {}
}
