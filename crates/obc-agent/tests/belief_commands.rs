//! "why do you believe X" and "forget that" from chat (personal-agents item 5,
//! 2026-11-02), through `AgentHandle::process` as the Telegram and Slack
//! channels call it. Each answered-from-world-memory case is paired with the
//! same words going to the model where they are not a belief command.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use obc_agent::handle::AgentHandle;
use obc_agent::world_context::WorldContextConfig;
use obc_agent::{Agent, AgentConfig};
use obc_memory::world::{Closure, Origin, WorldMemory};
use obc_memory::MemoryStore;
use obc_providers::{ChatCompletion, ChatMessage, ChatRole, Provider, ProviderConfig};
use obc_tool_api::Tool;
use serde_json::json;

/// A model that answers everything with the same line and counts its calls.
struct Model(Arc<Mutex<usize>>);

#[async_trait]
impl Provider for Model {
    fn name(&self) -> &str {
        "model"
    }
    async fn chat_completion(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Box<dyn Tool>],
        c: &ProviderConfig,
    ) -> anyhow::Result<ChatCompletion> {
        *self.0.lock().unwrap() += 1;
        Ok(ChatCompletion {
            message: "from the model".into(),
            tool_calls: vec![],
            provider: "model".into(),
            model: c.model.clone(),
            usage: None,
        })
    }
}

struct Bench {
    handle: Arc<AgentHandle>,
    world: Arc<WorldMemory>,
    memory: Arc<MemoryStore>,
    calls: Arc<Mutex<usize>>,
}

/// World memory holding a lock reading from `node-3` and a rollup that rests
/// on it, attached to the agent the way `main.rs` attaches it.
fn bench() -> Bench {
    let world = Arc::new(WorldMemory::open_in_memory().unwrap());
    let t = 1_700_000_000_000;
    let lock = world
        .observe_as(
            "front_door.lock",
            json!("locked"),
            t,
            t,
            "node-3",
            Origin::Observed,
        )
        .unwrap();
    world
        .observe_as_derived_from(
            "house.secure",
            json!(true),
            t,
            t,
            "rollup",
            Origin::Derived,
            &[lock.id],
        )
        .unwrap();
    let calls = Arc::new(Mutex::new(0));
    let memory = Arc::new(MemoryStore::open_in_memory().unwrap());
    let agent = Agent::new(
        AgentConfig {
            name: "belief-commands-test".into(),
            system_prompt: "test".into(),
            ..AgentConfig::default()
        },
        Arc::new(Model(Arc::clone(&calls))),
        Arc::clone(&memory),
        vec![],
    )
    .with_world_context(Arc::clone(&world), WorldContextConfig::default());
    let handle = Arc::new(AgentHandle::new(Arc::new(agent), ProviderConfig::default()));
    Bench {
        handle,
        world,
        memory,
        calls,
    }
}

#[tokio::test]
async fn why_is_answered_from_world_memory_with_its_source_and_dependents() {
    let b = bench();
    let r = b
        .handle
        .process("tg-42", "Why do you believe the front door is locked?")
        .await
        .unwrap();
    assert_eq!(r.provider, "world-memory");
    assert!(
        r.message.contains("front_door.lock = locked"),
        "{}",
        r.message
    );
    assert!(r.message.contains("`node-3` reported it"), "{}", r.message);
    assert!(r.message.contains("2023-11-14 22:13 UTC"), "{}", r.message);
    assert!(
        r.message.contains("Depends on it: #2 house.secure = true"),
        "{}",
        r.message
    );
    assert_eq!(*b.calls.lock().unwrap(), 0, "the model was asked");
    // The exchange is in the chat's history like any other.
    let history = b.memory.load_messages("tg-42").unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].role, ChatRole::Assistant);
    assert_eq!(history[1].content, r.message);

    // The same words from a scheduled task are not the operator asking.
    let r = b
        .handle
        .process_unattended(
            "scheduled-x",
            "Why do you believe the front door is locked?",
        )
        .await
        .unwrap();
    assert_eq!(r.message, "from the model");
    assert_eq!(*b.calls.lock().unwrap(), 1);

    // Nor is a question about something world memory does not hold.
    let r = b
        .handle
        .process("tg-42", "why do you think it will rain")
        .await
        .unwrap();
    assert_eq!(r.message, "from the model");
}

#[tokio::test]
async fn forget_that_withdraws_the_belief_just_looked_up_and_nothing_else() {
    let b = bench();
    let why = "why do you believe the front door is locked";

    // With nothing looked up in this chat, "forget that" goes to the model.
    let r = b.handle.process("tg-42", "forget that").await.unwrap();
    assert_eq!(r.message, "from the model");
    // So does a lookup followed by something else.
    b.handle.process("tg-42", why).await.unwrap();
    b.handle.process("tg-42", "thanks").await.unwrap();
    let r = b.handle.process("tg-42", "forget that").await.unwrap();
    assert_eq!(r.message, "from the model");
    // And a lookup in another chat.
    b.handle.process("slack-C1", why).await.unwrap();
    let r = b.handle.process("tg-42", "forget that").await.unwrap();
    assert_eq!(r.message, "from the model");
    // "forget" of something world memory does not hold.
    let r = b.handle.process("tg-42", "forget the milk").await.unwrap();
    assert_eq!(r.message, "from the model");
    assert!(b.world.current("front_door.lock").unwrap().is_some());
    let asked = *b.calls.lock().unwrap();

    // Right after the lookup, it is withdrawn.
    b.handle.process("tg-42", why).await.unwrap();
    let r = b.handle.process("tg-42", "Forget that.").await.unwrap();
    assert_eq!(r.provider, "world-memory");
    assert!(
        r.message.starts_with("Forgotten: front_door.lock"),
        "{}",
        r.message
    );
    assert!(
        r.message
            .contains("lost their grounds: #2 house.secure = true"),
        "{}",
        r.message
    );
    assert_eq!(*b.calls.lock().unwrap(), asked, "the model was asked");
    assert!(b.world.current("front_door.lock").unwrap().is_none());
    let last = b.world.history("front_door.lock").unwrap().pop().unwrap();
    assert_eq!(
        Closure::of(&last),
        Closure::Operator("forgotten on request in chat (tg-42)".into())
    );
    // A new reading is believed again.
    let t = 1_800_000_000_000;
    b.world
        .observe_as(
            "front_door.lock",
            json!("unlocked"),
            t,
            t,
            "node-3",
            Origin::Observed,
        )
        .unwrap();
    assert_eq!(
        b.world.current("front_door.lock").unwrap().unwrap().value,
        json!("unlocked")
    );
}
