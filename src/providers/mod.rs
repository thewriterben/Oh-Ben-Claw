//! Oh-Ben-Claw LLM provider adapters.
//!
//! Each provider adapter implements the `Provider` trait, which defines a
//! common interface for sending messages to an LLM and receiving responses.
//!
//! # Supported Providers
//!
//! | Provider    | Status  | Notes                                          |
//! |-------------|---------|------------------------------------------------|
//! | OpenAI      | Planned | GPT-4o, GPT-4, GPT-3.5-turbo                  |
//! | Anthropic   | Planned | Claude 3.5 Sonnet, Claude 3 Haiku              |
//! | Google      | Planned | Gemini 1.5 Pro, Gemini 1.5 Flash               |
//! | Ollama      | Planned | Local models (Llama 3, Mistral, etc.)          |
//! | Compatible  | Planned | Any OpenAI-compatible endpoint                 |
//! | OpenRouter  | Planned | Multi-provider routing                         |
//! | Reliable    | Planned | Automatic failover across providers            |
