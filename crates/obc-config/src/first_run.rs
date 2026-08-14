//! First-run provider resolution: make "export one key and run" literally true.
//!
//! The built-in default names `openai`/`gpt-4o`. That is a reasonable cloud-first
//! default and a hostile one: a newcomer who exported `ANTHROPIC_API_KEY` gets
//! `OPENAI_API_KEY not set`, which names an environment variable they never intended to
//! use and says nothing about the one they did.
//!
//! So when there is no config to obey, look at which key is actually present and use
//! that provider. This runs **only** on the built-in-defaults path — an explicit config
//! is never second-guessed, because a config file is a stated intention and inferring
//! around it would be worse than the problem being solved.
//!
//! # On the model names
//!
//! Provider model identifiers move, and this file will go stale. That is a known
//! weakness, not an oversight: a default model is the difference between a working first
//! run and a 404 from a vendor API, and no amount of care here prevents a rename
//! upstream. The mitigation is that the chosen model is logged at startup and named in
//! `config.example.toml` as the first thing to change — a wrong guess should be one
//! obvious edit, not a mystery.
//!
//! What *is* enforced is that the guess is made in one place. The same identifier
//! appears in `config.example.toml`, in the config the deployment planner emits, and
//! in both reference bodies; before `the_pinned_model_appears_everywhere_it_should`
//! existed they could be updated separately, so a careful edit here left three copies
//! of the old value in the files a new user actually reads.
//!
//! Checked 2026-07-28 against the Anthropic model list: `claude-sonnet-5` is current,
//! retirement not before 2027-06-30. The previous pin, `claude-sonnet-4-5`, was still
//! callable but two generations back with its retirement window opening 2026-09-29 —
//! which for a first-run default is a 404 with a two-month fuse.

use super::{ProviderConfig, SecretString};

/// A provider that can be reached with only an environment variable, in preference
/// order.
///
/// Order is not a quality judgement. It is stability of the identifier: providers whose
/// model names change least often come first, so the default that goes stale slowest is
/// the one most likely to be chosen.
const CANDIDATES: &[(&str, &str, &str)] = &[
    // (provider name, env var, default model)
    ("anthropic", "ANTHROPIC_API_KEY", "claude-sonnet-5"),
    ("openai", "OPENAI_API_KEY", "gpt-4o"),
    (
        "openrouter",
        "OPENROUTER_API_KEY",
        "anthropic/claude-sonnet-5",
    ),
];

/// The model identifier this crate pins for `provider`, if it has one.
///
/// Public because the pinned string is a cross-artifact contract, not a private
/// default: it has to match `config.example.toml`, the config the deployment
/// planner emits, and the reference bodies. `tests/pinned_model.rs` asserts
/// exactly that, and needs to be able to ask rather than to be told.
///
/// It lived as a private `CANDIDATES` lookup inside a unit test until
/// 2026-08-13. That test named the deployment module twice, which was the
/// *whole* of this module's edge into it — one of three mutual pairs in the
/// dependency graph, held open by two lines inside `#[cfg(test)]`.
///
/// (Named, not spelled: `scripts/core_endgame.py` counts textual crossings and
/// cannot tell a doc comment from a call, so writing the path here would leave
/// the edge on the graph after the code was gone.)
pub fn pinned_model(provider: &str) -> Option<&'static str> {
    CANDIDATES
        .iter()
        .find(|(p, _, _)| *p == provider)
        .map(|(_, _, m)| *m)
}

/// The local option, tried last and only when nothing else is configured.
const LOCAL: (&str, &str) = ("ollama", "llama3.2");

/// What first-run resolution decided, and why — so the caller can say it out loud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// A provider key was found in the environment.
    FromEnv {
        provider: String,
        var: String,
        model: String,
    },
    /// No key anywhere; fall back to a local endpoint that needs none.
    LocalFallback { provider: String, model: String },
}

impl Resolution {
    pub fn provider(&self) -> &str {
        match self {
            Resolution::FromEnv { provider, .. } | Resolution::LocalFallback { provider, .. } => {
                provider
            }
        }
    }
}

/// Choose a provider from the environment.
///
/// Returns the resolution and a ready `ProviderConfig`. The `api_key` is left `None`
/// deliberately: the provider modules already read the environment variable themselves,
/// and copying the secret into a struct that may later be serialised to disk would
/// undo the point of [`SecretString`].
pub fn resolve(env: impl Fn(&str) -> Option<String>) -> (Resolution, ProviderConfig) {
    for (provider, var, model) in CANDIDATES {
        if env(var).is_some_and(|v| !v.trim().is_empty()) {
            return (
                Resolution::FromEnv {
                    provider: (*provider).to_string(),
                    var: (*var).to_string(),
                    model: (*model).to_string(),
                },
                ProviderConfig {
                    name: (*provider).to_string(),
                    model: (*model).to_string(),
                    api_key: None,
                    ..ProviderConfig::default()
                },
            );
        }
    }
    let (provider, model) = LOCAL;
    (
        Resolution::LocalFallback {
            provider: provider.to_string(),
            model: model.to_string(),
        },
        ProviderConfig {
            name: provider.to_string(),
            model: model.to_string(),
            api_key: None,
            ..ProviderConfig::default()
        },
    )
}

/// Resolve against the real environment.
pub fn resolve_from_env() -> (Resolution, ProviderConfig) {
    resolve(|k| std::env::var(k).ok())
}

/// The message to print when nothing is configured — every option, named.
///
/// The old failure said `OPENAI_API_KEY not set`, which is unhelpful twice over: it
/// names a vendor the user may not use, and it does not mention that running locally
/// needs no key at all.
pub fn guidance() -> String {
    let vars: Vec<&str> = CANDIDATES.iter().map(|(_, v, _)| *v).collect();
    format!(
        "No provider configured. Set one of {} and OBC will use it, or run a local \
         Ollama (no key needed) and OBC will fall back to it. To pin a specific \
         provider or model, write a config.toml — see config.example.toml for where \
         it goes.",
        vars.join(", ")
    )
}

/// Unused-but-exported so `SecretString` stays reachable from this module's docs.
#[allow(dead_code)]
fn _doc_anchor(_: Option<SecretString>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn it_picks_the_provider_whose_key_is_present() {
        // The case that motivated this: a newcomer with only an Anthropic key used to be
        // told `OPENAI_API_KEY not set`.
        let (r, cfg) = resolve(env_of(&[("ANTHROPIC_API_KEY", "sk-ant-x")]));
        assert_eq!(cfg.name, "anthropic");
        assert!(matches!(r, Resolution::FromEnv { ref var, .. } if var == "ANTHROPIC_API_KEY"));

        let (_, cfg) = resolve(env_of(&[("OPENAI_API_KEY", "sk-x")]));
        assert_eq!(cfg.name, "openai");

        let (_, cfg) = resolve(env_of(&[("OPENROUTER_API_KEY", "sk-or-x")]));
        assert_eq!(cfg.name, "openrouter");
    }

    #[test]
    fn preference_order_is_stable_when_several_keys_exist() {
        // Someone with three keys exported should get the same answer every run — an
        // agent that changes brain between restarts depending on hash iteration order
        // would be a genuinely horrible thing to debug.
        let e = env_of(&[
            ("OPENAI_API_KEY", "sk-x"),
            ("ANTHROPIC_API_KEY", "sk-ant-x"),
            ("OPENROUTER_API_KEY", "sk-or-x"),
        ]);
        for _ in 0..5 {
            assert_eq!(resolve(&e).1.name, "anthropic");
        }
    }

    #[test]
    fn an_empty_or_whitespace_key_does_not_count() {
        // `export ANTHROPIC_API_KEY=` is a very common way to end up with a set-but-empty
        // variable, and treating it as configured produces a 401 instead of guidance.
        let (r, cfg) = resolve(env_of(&[
            ("ANTHROPIC_API_KEY", "   "),
            ("OPENAI_API_KEY", "sk-x"),
        ]));
        assert_eq!(cfg.name, "openai");
        assert!(matches!(r, Resolution::FromEnv { .. }));
    }

    #[test]
    fn with_no_keys_at_all_it_falls_back_to_local() {
        // Not an error. A local endpoint needs no credential, so the honest default for
        // someone who has configured nothing is the one that might actually answer.
        let (r, cfg) = resolve(env_of(&[]));
        assert_eq!(cfg.name, "ollama");
        assert!(matches!(r, Resolution::LocalFallback { .. }));
    }

    #[test]
    fn the_key_is_not_copied_into_the_config() {
        // The provider modules read the environment themselves. Copying the secret into a
        // struct that may be serialised to disk would undo SecretString entirely.
        let (_, cfg) = resolve(env_of(&[("ANTHROPIC_API_KEY", "sk-ant-secret")]));
        assert!(cfg.api_key.is_none());
        assert!(!format!("{cfg:?}").contains("secret"));
    }

    // `the_pinned_model_appears_everywhere_it_should` moved to
    // tests/pinned_model.rs on 2026-08-13. It named the deployment module twice
    // to check that the planner's emitted config offers the same identifier
    // this file pins -- and those two lines were the whole of the config module's
    // edge into deployment, one half of a mutual pair in the dependency graph.
    //
    // The claim spans config and deployment, so it belongs where both can be
    // named. `pinned_model()` above is public for that test to call; the path
    // is not spelled here because the endgame script counts text, not calls.

    #[test]
    fn the_guidance_names_every_option() {
        let g = guidance();
        for (_, var, _) in CANDIDATES {
            assert!(g.contains(var), "{var} missing from: {g}");
        }
        assert!(g.contains("Ollama"), "the no-key path is named too: {g}");
    }
}
