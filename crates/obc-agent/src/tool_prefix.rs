//! Which tool schemas ride in every prompt (`[agent.tools]`, 2026-09-28).
//!
//! Two months of bench logs (27 Jul → 28 Sep 2026): 152 executed tool calls,
//! 18 distinct tools out of the 31 registered, and the top five made 109 of
//! them. Every prompt still carried all 31 schemas — about 26 k characters,
//! half of a cold prefix — because the registry and the prompt were the same
//! list. This module separates them.
//!
//! `[agent.tools] full = [...]` names the tools whose full schema goes into
//! every prompt. The rest stay **registered** — the execution chokepoint,
//! skill replay, `mcp-serve` and `a2a-serve` see them as before — but are only
//! *listed* in the system prompt, one line each, under "Tools on the shelf",
//! with a small built-in `load_tools` the model calls to pull a schema into the
//! prompt for the rest of the session. The list is empty by default, which is
//! the old behaviour: everything in every prompt.
//!
//! Cache stability: the catalog is a function of the config and the registry,
//! not of what a session has loaded, so the system message stays byte-stable
//! across turns; a loaded schema joins the `tools` array, which providers
//! cache separately.

use obc_tool_api::{RiskClass, Tool, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

/// The name of the built-in that pulls a shelved schema into the prompt.
pub const LOAD_TOOLS: &str = "load_tools";

/// `[agent.tools]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolPrefixConfig {
    /// Tools whose full schema is in every prompt. Empty (the default) means
    /// all of them, as before this table existed.
    #[serde(default)]
    pub full: Vec<String>,
    /// List the shelved tools in the system prompt and offer `load_tools`.
    /// Off means shelved tools are simply absent from the prompt (they stay
    /// callable by name through the chokepoint). Default on.
    #[serde(default = "default_true")]
    pub catalog: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ToolPrefixConfig {
    fn default() -> Self {
        Self {
            full: Vec::new(),
            catalog: true,
        }
    }
}

impl ToolPrefixConfig {
    /// Whether any tool is kept out of the prompt.
    pub fn shelves(&self) -> bool {
        !self.full.is_empty()
    }

    /// Whether this tool's schema is always in the prompt.
    pub fn is_full(&self, name: &str) -> bool {
        !self.shelves() || self.full.iter().any(|n| n == name)
    }

    /// Names in `full` that no registered tool carries — a typo in the
    /// config, or a tool this build does not register. Logged once by the
    /// agent; never fatal, the name is simply ignored.
    pub fn unknown_full<'a>(&'a self, registry: &[Arc<dyn Tool>]) -> Vec<&'a str> {
        self.full
            .iter()
            .map(String::as_str)
            .filter(|n| !registry.iter().any(|t| t.name() == *n))
            .collect()
    }
}

/// The tools for one turn: the ones the model sees as schemas, and the names
/// of the ones it does not.
pub struct TurnTools {
    pub active: Vec<Box<dyn Tool>>,
    pub shelved: Vec<String>,
}

/// Split the registry for a turn. `loaded` is what this session has already
/// pulled in with `load_tools`; those ride along as full schemas too.
pub fn split(
    cfg: &ToolPrefixConfig,
    registry: &[Arc<dyn Tool>],
    loaded: &BTreeSet<String>,
) -> TurnTools {
    if !cfg.shelves() {
        return TurnTools {
            active: registry
                .iter()
                .map(|t| Box::new(Arc::clone(t)) as Box<dyn Tool>)
                .collect(),
            shelved: Vec::new(),
        };
    }
    let mut active: Vec<Box<dyn Tool>> = Vec::with_capacity(registry.len());
    let mut shelved = Vec::new();
    for t in registry {
        if cfg.is_full(t.name()) || loaded.contains(t.name()) {
            active.push(Box::new(Arc::clone(t)) as Box<dyn Tool>);
        } else {
            shelved.push(t.name().to_string());
        }
    }
    if cfg.catalog && !shelved.is_empty() {
        active.push(Box::new(LoadTools));
    }
    shelved.sort();
    TurnTools { active, shelved }
}

/// The "Tools on the shelf" block for the system prompt, or `None` when
/// nothing is shelved or the catalog is off. Depends on the config and the
/// registry only, so it is the same bytes every turn of every session.
pub fn catalog(cfg: &ToolPrefixConfig, registry: &[Arc<dyn Tool>]) -> Option<String> {
    if !cfg.shelves() || !cfg.catalog {
        return None;
    }
    let mut rows: Vec<(String, String)> = registry
        .iter()
        .filter(|t| !cfg.is_full(t.name()))
        .map(|t| (t.name().to_string(), summary(t.description())))
        .collect();
    if rows.is_empty() {
        return None;
    }
    rows.sort();
    let mut out = String::from(
        "## Tools on the shelf\n\n\
         These tools exist but their schemas are not in this prompt, to keep it \
         small. To use one, call `load_tools` with its name; the schema arrives \
         for your next step and stays for the session. Load only what the \
         request needs.\n\n",
    );
    for (name, what) in rows {
        out.push_str("- `");
        out.push_str(&name);
        out.push_str("` — ");
        out.push_str(&what);
        out.push('\n');
    }
    Some(out)
}

/// The first sentence of a description, at most `MAX` characters.
fn summary(description: &str) -> String {
    const MAX: usize = 110;
    let first_line = description.trim().lines().next().unwrap_or("").trim();
    let sentence = match first_line.find(". ") {
        Some(i) => &first_line[..=i],
        None => first_line,
    };
    if sentence.chars().count() <= MAX {
        return sentence.to_string();
    }
    let cut: String = sentence.chars().take(MAX - 1).collect();
    let cut = cut.trim_end_matches([' ', ',', ';', ':']);
    format!("{cut}…")
}

/// The built-in the model calls to pull a shelved schema in. The agent loop
/// answers it itself (it has to change the prompt for the next step), so
/// `execute` is only ever reached if something bypasses the loop.
pub struct LoadTools;

#[async_trait::async_trait]
impl Tool for LoadTools {
    fn name(&self) -> &str {
        // A literal, not the constant: `scripts/check_physical_tools.py`
        // classifies tools by their literal name (a computed one must declare
        // its risk explicitly). `load_tools_is_named_by_the_constant` keeps
        // the two equal.
        "load_tools"
    }

    /// Prompt shaping only: no hardware, no side effect beyond this session's
    /// tool list. Declared rather than inherited so the risk is stated where
    /// the tool is, as every built-in does.
    fn risk_class(&self) -> RiskClass {
        RiskClass::default()
    }

    fn description(&self) -> &str {
        "Bring one or more shelved tools (see \"Tools on the shelf\") into the \
         prompt so you can call them in your next step. Read-only: loads schemas, \
         runs nothing."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "names": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Tool names exactly as listed on the shelf."
                }
            },
            "required": ["names"]
        })
    }

    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        Ok(ToolResult::err(
            "load_tools is answered by the agent loop; calling it directly does nothing",
        ))
    }
}

/// The tool names in a `load_tools` call. Tolerant of the shapes small models
/// produce: `{"names":["a","b"]}`, `{"names":"a, b"}`, `{"name":"a"}`, or a
/// bare string.
pub fn parse_names(args: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::String(args.to_string()));
    let raw = match &v {
        Value::Object(m) => m
            .get("names")
            .or_else(|| m.get("name"))
            .or_else(|| m.get("tools"))
            .or_else(|| m.get("tool"))
            .cloned()
            .unwrap_or(Value::Null),
        other => other.clone(),
    };
    let mut names: Vec<String> = match raw {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|i| i.as_str().map(str::to_string))
            .collect(),
        Value::String(s) => s
            .split([',', ' ', '\n'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    names.iter_mut().for_each(|n| {
        *n = n.trim_matches(['`', '"', '\'']).to_string();
    });
    names.retain(|n| !n.is_empty());
    names.dedup();
    names
}

/// What a `load_tools` call did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LoadOutcome {
    pub loaded: Vec<String>,
    pub already: Vec<String>,
    pub unknown: Vec<String>,
}

impl LoadOutcome {
    /// The tool result the model reads.
    pub fn message(&self) -> String {
        let mut parts = Vec::new();
        if !self.loaded.is_empty() {
            parts.push(format!(
                "Loaded: {}. Call them in your next step.",
                self.loaded.join(", ")
            ));
        }
        if !self.already.is_empty() {
            parts.push(format!("Already available: {}.", self.already.join(", ")));
        }
        if !self.unknown.is_empty() {
            parts.push(format!(
                "Not a tool here: {}. Use a name from the shelf list.",
                self.unknown.join(", ")
            ));
        }
        if parts.is_empty() {
            parts.push("No tool names given; pass `names` from the shelf list.".to_string());
        }
        parts.join(" ")
    }
}

/// Apply a `load_tools` call to a session's loaded set. `registry` is every
/// registered tool name; `active` the names already in the prompt this turn.
pub fn apply_load(
    names: &[String],
    registry: &BTreeSet<String>,
    active: &BTreeSet<String>,
    loaded: &mut BTreeSet<String>,
) -> LoadOutcome {
    let mut out = LoadOutcome::default();
    for n in names {
        if n == LOAD_TOOLS || active.contains(n) {
            out.already.push(n.clone());
        } else if registry.contains(n) {
            if loaded.insert(n.clone()) {
                out.loaded.push(n.clone());
            } else {
                out.already.push(n.clone());
            }
        } else {
            out.unknown.push(n.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy(&'static str, &'static str);

    #[async_trait::async_trait]
    impl Tool for Dummy {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            self.1
        }
        fn risk_class(&self) -> RiskClass {
            RiskClass::default()
        }
        async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
            Ok(ToolResult::ok("ok"))
        }
    }

    #[test]
    fn load_tools_is_named_by_the_constant() {
        assert_eq!(LoadTools.name(), LOAD_TOOLS);
    }

    fn registry() -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(Dummy(
                "mesh_status",
                "Mesh nodes and their health. Long tail.",
            )),
            Arc::new(Dummy("shell", "Run a shell command in the sandbox.")),
            Arc::new(Dummy(
                "gnss_fix",
                "Latest GNSS fix from a node.\nMore lines.",
            )),
            Arc::new(Dummy("ota_update", "Push firmware over the air")),
        ]
    }

    fn names(tools: &[Box<dyn Tool>]) -> Vec<&str> {
        tools.iter().map(|t| t.name()).collect()
    }

    #[test]
    fn the_default_is_everything_and_no_catalog() {
        let cfg = ToolPrefixConfig::default();
        let reg = registry();
        let t = split(&cfg, &reg, &BTreeSet::new());
        assert_eq!(
            names(&t.active),
            ["mesh_status", "shell", "gnss_fix", "ota_update"]
        );
        assert!(t.shelved.is_empty());
        assert!(catalog(&cfg, &reg).is_none());
    }

    #[test]
    fn a_full_list_shelves_the_rest_and_adds_load_tools() {
        let cfg = ToolPrefixConfig {
            full: vec!["shell".into(), "mesh_status".into()],
            catalog: true,
        };
        let reg = registry();
        let t = split(&cfg, &reg, &BTreeSet::new());
        assert_eq!(names(&t.active), ["mesh_status", "shell", LOAD_TOOLS]);
        assert_eq!(t.shelved, ["gnss_fix", "ota_update"]);
        let text = catalog(&cfg, &reg).unwrap();
        assert!(text.starts_with("## Tools on the shelf"));
        assert!(text.contains("- `gnss_fix` — Latest GNSS fix from a node."));
        assert!(text.contains("- `ota_update` — Push firmware over the air"));
        assert!(!text.contains("`shell`"), "full tools are not on the shelf");
        assert!(!text.contains("More lines"), "one line per tool");
    }

    #[test]
    fn a_loaded_tool_rides_along_but_the_catalog_does_not_change() {
        let cfg = ToolPrefixConfig {
            full: vec!["shell".into()],
            catalog: true,
        };
        let reg = registry();
        let before = catalog(&cfg, &reg).unwrap();
        let loaded: BTreeSet<String> = ["gnss_fix".to_string()].into();
        let t = split(&cfg, &reg, &loaded);
        assert_eq!(names(&t.active), ["shell", "gnss_fix", LOAD_TOOLS]);
        assert_eq!(t.shelved, ["mesh_status", "ota_update"]);
        assert_eq!(catalog(&cfg, &reg).unwrap(), before, "cache-stable");
    }

    #[test]
    fn catalog_off_shelves_silently() {
        let cfg = ToolPrefixConfig {
            full: vec!["shell".into()],
            catalog: false,
        };
        let reg = registry();
        let t = split(&cfg, &reg, &BTreeSet::new());
        assert_eq!(names(&t.active), ["shell"]);
        assert_eq!(t.shelved.len(), 3);
        assert!(catalog(&cfg, &reg).is_none());
    }

    #[test]
    fn unknown_full_names_are_reported() {
        let cfg = ToolPrefixConfig {
            full: vec!["shell".into(), "shel".into()],
            catalog: true,
        };
        assert_eq!(cfg.unknown_full(&registry()), ["shel"]);
    }

    #[test]
    fn names_are_parsed_from_the_shapes_models_produce() {
        assert_eq!(parse_names(r#"{"names":["a","b"]}"#), ["a", "b"]);
        assert_eq!(parse_names(r#"{"names":"a, b"}"#), ["a", "b"]);
        assert_eq!(parse_names(r#"{"name":"`a`"}"#), ["a"]);
        assert_eq!(parse_names(r#"{"tools":["a"]}"#), ["a"]);
        assert_eq!(parse_names("a b"), ["a", "b"]);
        assert!(parse_names("{}").is_empty());
        assert!(parse_names(r#"{"names":[]}"#).is_empty());
    }

    #[test]
    fn apply_load_sorts_names_into_loaded_already_unknown() {
        let registry: BTreeSet<String> = ["shell", "gnss_fix", "ota_update"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let active: BTreeSet<String> = ["shell", LOAD_TOOLS]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut loaded = BTreeSet::new();
        let names: Vec<String> = ["gnss_fix", "shell", "nope", LOAD_TOOLS]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = apply_load(&names, &registry, &active, &mut loaded);
        assert_eq!(out.loaded, ["gnss_fix"]);
        assert_eq!(out.already, ["shell", LOAD_TOOLS]);
        assert_eq!(out.unknown, ["nope"]);
        assert!(loaded.contains("gnss_fix"));
        let again = apply_load(&["gnss_fix".to_string()], &registry, &active, &mut loaded);
        assert_eq!(again.already, ["gnss_fix"]);
        assert!(again.loaded.is_empty());
        assert!(out
            .message()
            .starts_with("Loaded: gnss_fix. Call them in your next step."));
        assert!(out.message().contains("Not a tool here: nope."));
        assert_eq!(
            apply_load(&[], &registry, &active, &mut loaded).message(),
            "No tool names given; pass `names` from the shelf list."
        );
    }

    #[test]
    fn the_config_table_parses_and_rejects_typos() {
        let cfg: ToolPrefixConfig =
            toml::from_str("full = [\"shell\", \"mesh_status\"]\ncatalog = false\n").unwrap();
        assert_eq!(cfg.full, ["shell", "mesh_status"]);
        assert!(!cfg.catalog);
        let bad: Result<ToolPrefixConfig, _> = toml::from_str("ful = [\"shell\"]\n");
        assert!(
            bad.is_err(),
            "an unknown key is a load failure, not a silent no-op"
        );
        let empty: ToolPrefixConfig = toml::from_str("").unwrap();
        assert_eq!(empty, ToolPrefixConfig::default());
    }

    #[test]
    fn summaries_are_one_sentence_and_bounded() {
        assert_eq!(summary("Short. Then more."), "Short.");
        assert_eq!(summary("  Two lines\nsecond"), "Two lines");
        let long = "x".repeat(300);
        let s = summary(&long);
        assert!(s.chars().count() <= 110);
        assert!(s.ends_with('…'));
    }
}
