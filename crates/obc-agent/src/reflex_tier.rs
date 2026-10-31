//! The reflex tier (Phase 20 plan P3): the small local model behind a reflex
//! rule's `model` action.
//!
//! [`ProviderReflexModel`] adapts any [`Provider`] to [`obc_reflex::ReflexModel`]:
//! one user message, no system prompt, no history, no tools. The controller
//! puts the latency ceiling around it; this side keeps the model registry
//! honest. A model the registry has freshly marked down is not called at all,
//! so a dead server costs a rule one miss per recheck interval rather than one
//! ceiling per tick, and every answer, error and timeout is recorded there,
//! which writes `edge.model.<name>` when the state changes.

use std::sync::Arc;

use obc_providers::model_registry::ModelRegistry;
use obc_providers::{ChatMessage, ChatRole, Provider, ProviderConfig};

/// A [`Provider`] answering reflex classifications.
pub struct ProviderReflexModel {
    provider: Arc<dyn Provider>,
    config: ProviderConfig,
    models: Option<Arc<ModelRegistry>>,
}

impl ProviderReflexModel {
    /// Answer with `provider` under `config` (its model, URL, temperature).
    pub fn new(provider: Arc<dyn Provider>, config: ProviderConfig) -> Self {
        Self {
            provider,
            config,
            models: None,
        }
    }

    /// Consult and update the edge node's model registry.
    pub fn with_registry(mut self, models: Arc<ModelRegistry>) -> Self {
        self.models = Some(models);
        self
    }

    fn record(&self, healthy: bool) {
        if let Some(m) = &self.models {
            m.record_health(&self.config.model, healthy, crate::now_ms());
        }
    }
}

#[async_trait::async_trait]
impl obc_reflex::ReflexModel for ProviderReflexModel {
    // The reflex tier names a model by its model id, not its provider.
    #[allow(clippy::misnamed_getters)]
    fn name(&self) -> &str {
        &self.config.model
    }

    async fn complete(&self, prompt: &str) -> anyhow::Result<String> {
        if let Some(m) = &self.models {
            if !m.usable(&self.config.model, crate::now_ms()) {
                anyhow::bail!("{} is down; waiting for its recheck", self.config.model);
            }
        }
        let messages = [ChatMessage {
            role: ChatRole::User,
            content: prompt.to_string(),
        }];
        match self
            .provider
            .chat_completion(&messages, &[], &self.config)
            .await
        {
            Ok(c) => {
                self.record(true);
                Ok(c.message)
            }
            Err(e) => {
                self.record(false);
                Err(e)
            }
        }
    }

    fn timed_out(&self) {
        self.record(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_reflex::ReflexModel;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Replies with `reply` (or fails on `"FAIL"`), recording what it was sent.
    struct Scripted {
        reply: &'static str,
        calls: AtomicUsize,
        seen: Mutex<Vec<(usize, usize)>>,
    }

    #[async_trait::async_trait]
    impl Provider for Scripted {
        fn name(&self) -> &str {
            "ollama"
        }
        async fn chat_completion(
            &self,
            m: &[ChatMessage],
            t: &[Box<dyn obc_tool_api::Tool>],
            c: &ProviderConfig,
        ) -> anyhow::Result<obc_providers::ChatCompletion> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().unwrap().push((m.len(), t.len()));
            if self.reply == "FAIL" {
                anyhow::bail!("connection refused");
            }
            Ok(obc_providers::ChatCompletion {
                message: self.reply.to_string(),
                tool_calls: vec![],
                provider: "ollama".into(),
                model: c.model.clone(),
                usage: None,
            })
        }
    }

    fn setup(
        reply: &'static str,
    ) -> (
        ProviderReflexModel,
        Arc<Scripted>,
        Arc<obc_memory::world::WorldMemory>,
    ) {
        let cfg = crate::EdgeConfig {
            local_model: Some("gemma3n".into()),
            ..Default::default()
        }
        .reflex_provider()
        .unwrap();
        let world = Arc::new(obc_memory::world::WorldMemory::open_in_memory().unwrap());
        let models = Arc::new(
            obc_providers::model_registry::registry_from_providers(
                std::slice::from_ref(&cfg),
                60_000,
            )
            .with_world(Arc::clone(&world)),
        );
        let provider = Arc::new(Scripted {
            reply,
            calls: AtomicUsize::new(0),
            seen: Mutex::new(Vec::new()),
        });
        let model = ProviderReflexModel::new(provider.clone(), cfg).with_registry(models);
        (model, provider, world)
    }

    #[tokio::test]
    async fn one_message_no_tools_and_the_model_is_marked_up() {
        let (model, provider, world) = setup("person");
        assert_eq!(model.name(), "gemma3n");
        assert_eq!(model.complete("Person or animal?").await.unwrap(), "person");
        assert_eq!(provider.seen.lock().unwrap().as_slice(), &[(1, 0)]);
        let fact = world.current("edge.model.gemma3n").unwrap().unwrap();
        assert_eq!(
            fact.value,
            serde_json::json!({"status": "up", "local": true})
        );
    }

    #[tokio::test]
    async fn a_down_model_is_recorded_and_not_called_again_until_its_recheck() {
        let (model, provider, world) = setup("FAIL");
        assert!(model.complete("Person or animal?").await.is_err());
        let fact = world.current("edge.model.gemma3n").unwrap().unwrap();
        assert_eq!(fact.value["status"], "down");
        let err = model.complete("Person or animal?").await.unwrap_err();
        assert!(err.to_string().contains("down"), "{err}");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_timeout_marks_the_model_down() {
        let (model, _provider, world) = setup("person");
        model.timed_out();
        let fact = world.current("edge.model.gemma3n").unwrap().unwrap();
        assert_eq!(fact.value["status"], "down");
    }
}
