//! Hook runner — dispatches lifecycle events to registered handlers.

use crate::hooks::traits::{HookHandler, HookResult};
use serde_json::Value;
use std::cmp::Reverse;

/// Dispatches lifecycle events to a sorted list of [`HookHandler`]s.
pub struct HookRunner {
    handlers: Vec<Box<dyn HookHandler>>,
}

impl HookRunner {
    /// Create an empty `HookRunner`.
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    /// Register a handler, then re-sort all handlers by descending priority.
    pub fn register(&mut self, handler: Box<dyn HookHandler>) {
        self.handlers.push(handler);
        self.handlers.sort_by_key(|h| Reverse(h.priority()));
    }

    /// Notify all handlers that a session has started (fire-and-forget).
    pub fn fire_session_start(&self, session_id: &str, channel: &str) {
        for h in &self.handlers {
            h.on_session_start(session_id, channel);
        }
    }

    /// Notify all handlers that a session has ended (fire-and-forget).
    pub fn fire_session_end(&self, session_id: &str) {
        for h in &self.handlers {
            h.on_session_end(session_id);
        }
    }

    /// Dispatch `on_message_received` sequentially; short-circuits on the first [`HookResult::Cancel`].
    pub fn fire_message_received(&self, session_id: &str, content: &str) -> HookResult {
        for h in &self.handlers {
            if let HookResult::Cancel { reason } = h.on_message_received(session_id, content) {
                return HookResult::Cancel { reason };
            }
        }
        HookResult::Continue
    }

    /// Dispatch `on_tool_call` sequentially; short-circuits on the first [`HookResult::Cancel`].
    pub fn fire_tool_call(&self, tool_name: &str, args: &Value) -> HookResult {
        for h in &self.handlers {
            if let HookResult::Cancel { reason } = h.on_tool_call(tool_name, args) {
                return HookResult::Cancel { reason };
            }
        }
        HookResult::Continue
    }

    /// Notify all handlers of a tool result (fire-and-forget).
    pub fn fire_tool_result(&self, tool_name: &str, result: &str) {
        for h in &self.handlers {
            h.on_tool_result(tool_name, result);
        }
    }

    /// Notify all handlers of an agent response (fire-and-forget).
    pub fn fire_agent_response(&self, session_id: &str, response: &str) {
        for h in &self.handlers {
            h.on_agent_response(session_id, response);
        }
    }

    /// Notify all handlers that the agent has started (fire-and-forget).
    pub fn fire_agent_start(&self) {
        for h in &self.handlers {
            h.on_agent_start();
        }
    }

    /// Notify all handlers that the agent has stopped (fire-and-forget).
    pub fn fire_agent_stop(&self) {
        for h in &self.handlers {
            h.on_agent_stop();
        }
    }
}

impl Default for HookRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::traits::{HookHandler, HookResult};
    use parking_lot::Mutex;
    use std::sync::Arc;

    struct TrackingHook {
        calls: Arc<Mutex<Vec<String>>>,
        priority: i32,
    }

    impl HookHandler for TrackingHook {
        fn priority(&self) -> i32 {
            self.priority
        }

        fn on_session_start(&self, session_id: &str, _channel: &str) {
            self.calls
                .lock()
                .push(format!("session_start:{}", session_id));
        }

        fn on_message_received(&self, _session_id: &str, content: &str) -> HookResult {
            self.calls.lock().push(format!("msg:{}", content));
            HookResult::Continue
        }

        fn on_tool_call(&self, tool_name: &str, _args: &serde_json::Value) -> HookResult {
            self.calls.lock().push(format!("tool:{}", tool_name));
            HookResult::Continue
        }
    }

    struct CancellingHook;

    impl HookHandler for CancellingHook {
        fn on_message_received(&self, _session_id: &str, _content: &str) -> HookResult {
            HookResult::Cancel {
                reason: "blocked".to_string(),
            }
        }
        fn on_tool_call(&self, _tool_name: &str, _args: &serde_json::Value) -> HookResult {
            HookResult::Cancel {
                reason: "blocked".to_string(),
            }
        }
    }

    #[test]
    fn session_start_dispatched() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let hook = TrackingHook {
            calls: calls.clone(),
            priority: 0,
        };
        let mut runner = HookRunner::new();
        runner.register(Box::new(hook));
        runner.fire_session_start("s1", "cli");
        assert!(calls.lock().contains(&"session_start:s1".to_string()));
    }

    #[test]
    fn message_cancel_short_circuits() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tracking = TrackingHook {
            calls: calls.clone(),
            priority: -1,
        };
        let mut runner = HookRunner::new();
        runner.register(Box::new(CancellingHook));
        runner.register(Box::new(tracking));
        let result = runner.fire_message_received("s1", "hello");
        assert!(matches!(result, HookResult::Cancel { .. }));
        // TrackingHook has lower priority — should NOT have been called
        assert!(!calls.lock().iter().any(|c| c.starts_with("msg:")));
    }

    #[test]
    fn tool_cancel_short_circuits() {
        let mut runner = HookRunner::new();
        runner.register(Box::new(CancellingHook));
        let result = runner.fire_tool_call("shell", &serde_json::json!({}));
        assert!(matches!(result, HookResult::Cancel { .. }));
    }

    #[test]
    fn priority_ordering() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let low = TrackingHook {
            calls: calls.clone(),
            priority: 1,
        };
        let high = TrackingHook {
            calls: calls.clone(),
            priority: 10,
        };
        let mut runner = HookRunner::new();
        runner.register(Box::new(low));
        runner.register(Box::new(high));
        runner.fire_session_start("s1", "cli");
        let log = calls.lock().clone();
        // Both should fire
        assert_eq!(log.len(), 2);
    }
}
