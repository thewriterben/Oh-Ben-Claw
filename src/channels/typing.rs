//! Typing indicator helpers for chat channels.
//!
//! Each channel that supports typing indicators implements the
//! [`TypingIndicator`] trait.  A [`TypingTask`] spawns a background task that
//! periodically refreshes the typing status while the agent is processing a
//! message, then cancels automatically when dropped.
//!
//! # Usage
//!
//! ```rust,ignore
//! let typing = TypingTask::start(|| async { channel.send_typing(chat_id).await });
//! let response = agent.process(&session, &text, &config).await?;
//! drop(typing); // cancels the background task
//! ```

use std::future::Future;
use std::time::Duration;

/// Handle to a background typing-indicator task.  The task is automatically
/// cancelled when this value is dropped.
pub struct TypingTask {
    _handle: tokio::task::JoinHandle<()>,
}

impl TypingTask {
    /// Spawn a task that calls `refresh` every `interval_secs` seconds until
    /// this handle is dropped.
    ///
    /// `refresh` is an async closure that should call the platform's "typing"
    /// API endpoint.  Any errors are silently ignored (typing indicators are
    /// best-effort).
    pub fn start<F, Fut>(interval_secs: u64, mut refresh: F) -> Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(async move {
            let delay = Duration::from_secs(interval_secs);
            // Fire immediately, then repeat.
            loop {
                refresh().await;
                tokio::time::sleep(delay).await;
            }
        });

        Self { _handle: handle }
    }
}

impl Drop for TypingTask {
    fn drop(&mut self) {
        self._handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn typing_task_fires_at_least_once() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let task = TypingTask::start(1, move || {
            let c = counter_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
            }
        });

        // Give it a moment to fire
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(task); // cancel

        assert!(counter.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn typing_task_cancelled_on_drop() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let task = TypingTask::start(60, move || {
            // 60-second interval — will only fire once immediately
            let c = counter_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        let count_before = counter.load(Ordering::SeqCst);
        drop(task);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let count_after = counter.load(Ordering::SeqCst);

        // No additional calls after drop
        assert_eq!(count_before, count_after);
    }
}
