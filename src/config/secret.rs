//! A string that does not print itself.
//!
//! `ProviderConfig.api_key` was a plain `Option<String>` on a struct deriving `Debug`.
//! Nothing logged it today — but "nothing logs it today" is a property of the current
//! call sites, not of the type, and it is one `debug!("{config:?}")` away from being
//! false. A public project acquires those call sites from people who have never read
//! this file.
//!
//! So the redaction lives in the type. `Debug` and `Display` print a placeholder; getting
//! the real value requires calling [`SecretString::expose`], which is greppable and reads
//! like what it is at the point of use.
//!
//! **Serde is deliberately *not* redacted.** Serialisation is how a config round-trips to
//! disk, and a `save` that quietly replaced a key with `***` would corrupt the file it was
//! trying to preserve. The distinction that matters: serialisation is something a caller
//! asked for, and `Debug` is something that happens to code by accident.

use serde::{Deserialize, Serialize};

/// A secret that redacts itself in debug and display output.
///
/// Deserialises from and serialises to a plain string, so config files are unchanged.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretString(String);

const REDACTED: &str = "***redacted***";

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The real value. Named to be conspicuous in review and in `grep`.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// A safe fingerprint for diagnostics: enough to tell two keys apart, not enough to
    /// use one. Four leading characters is what every provider's own console shows.
    pub fn hint(&self) -> String {
        let n = self.0.chars().count();
        if n == 0 {
            return "(empty)".into();
        }
        let head: String = self.0.chars().take(4).collect();
        format!("{head}… ({n} chars)")
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

impl std::fmt::Display for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

impl From<String> for SecretString {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SecretString {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_never_show_the_value() {
        let s = SecretString::new("sk-ant-api03-REAL-KEY-MATERIAL");
        assert_eq!(format!("{s:?}"), REDACTED);
        assert_eq!(format!("{s}"), REDACTED);
        assert!(!format!("{s:?}").contains("REAL"));
    }

    #[test]
    fn a_struct_holding_one_is_safe_to_debug() {
        // The actual hazard: not printing the secret directly, but printing something
        // that contains it. This is the case `#[derive(Debug)]` on a config gets wrong.
        // Read only by the derived Debug, which `dead_code` does not count as a use.
        // That is the whole point of the test — narrow allow, stated reason.
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Holder {
            name: String,
            key: Option<SecretString>,
        }
        let h = Holder {
            name: "anthropic".into(),
            key: Some(SecretString::new("sk-ant-SENSITIVE")),
        };
        let printed = format!("{h:?}");
        assert!(
            printed.contains("anthropic"),
            "non-secret fields still readable"
        );
        assert!(
            !printed.contains("SENSITIVE"),
            "but not this one: {printed}"
        );
    }

    #[test]
    fn serde_round_trips_the_real_value() {
        // Redacting here would corrupt the config file a `save` was trying to preserve.
        // Serialisation is asked for; Debug happens by accident. Only the accident is
        // defended against.
        let s = SecretString::new("sk-real");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"sk-real\"");
        let back: SecretString = serde_json::from_str(&json).unwrap();
        assert_eq!(back.expose(), "sk-real");
    }

    #[test]
    fn it_deserialises_from_a_bare_toml_string() {
        // Existing configs must keep working unchanged.
        #[derive(Deserialize)]
        struct Cfg {
            api_key: Option<SecretString>,
        }
        let cfg: Cfg = toml::from_str("api_key = \"sk-inline\"").unwrap();
        assert_eq!(cfg.api_key.unwrap().expose(), "sk-inline");
    }

    #[test]
    fn inline_keys_are_found_including_in_fallbacks() {
        // Fallbacks are the ones that get forgotten: edited once, never looked at again.
        use crate::config::{inline_secret_providers, Config, ProviderConfig};
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

    #[test]
    fn the_hint_identifies_without_disclosing() {
        let s = SecretString::new("sk-ant-api03-abcdefghijklmnop");
        let h = s.hint();
        assert!(h.starts_with("sk-a"), "{h}");
        assert!(!h.contains("abcdefgh"), "{h}");
        assert!(h.contains("29 chars"), "{h}");
        assert_eq!(SecretString::new("").hint(), "(empty)");
    }
}
