//! An inline API key anywhere in the provider chain must be found, including
//! in a fallback.
//!
//! Fallbacks are the ones that get forgotten: edited once, never looked at
//! again. `inline_secret_providers` walks them, and this asserts it.
//!
//! It lived in `src/config/secret.rs`'s test module until 2026-08-13, when
//! `SecretString` moved to `obc-safety` — it is a secret-hygiene primitive, and
//! that crate already owns the vault. The type moved; this test could not
//! follow it, because it names the root `Config` and `ProviderConfig` to build
//! a realistic chain, and neither exists inside the crate.
//!
//! Which is the right outcome rather than an inconvenience. A test spanning the
//! redacting type and the configuration that carries it belongs where both can
//! be named. The unit tests that only exercise `SecretString` itself — redaction
//! in `Debug`, round-tripping through serde, the hint — went with it.

use obc_safety::SecretString;
use oh_ben_claw::config::{inline_secret_providers, Config, ProviderConfig};

#[test]
fn inline_keys_are_found_including_in_fallbacks() {
    let mut cfg = Config::default();
    cfg.provider.name = "ollama".into();
    cfg.provider.api_key = None;
    cfg.provider.fallbacks = vec![ProviderConfig {
        name: "anthropic".into(),
        api_key: Some(SecretString::new("sk-ant-forgotten")),
        ..ProviderConfig::default()
    }];
    let found = inline_secret_providers(&cfg);
    assert_eq!(found, vec!["provider.fallbacks[0] (anthropic)".to_string()]);

    // And an empty string is not a credential.
    cfg.provider.fallbacks[0].api_key = Some(SecretString::new(""));
    assert!(inline_secret_providers(&cfg).is_empty());
}
