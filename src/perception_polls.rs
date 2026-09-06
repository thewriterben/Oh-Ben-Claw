//! Generic MCP → world-memory polls: `[[perception.polls]]`.
//!
//! Until 2026-09-06 exactly one MCP server could be *perceived*: ClawCam, through
//! `[perception.clawcam_poll]`, which knows what a detection is and does a great
//! deal with that knowledge (review states, node health, audio, spatial fusion,
//! the conscience gate). Everything else an MCP server can report — a printer's
//! state, a design engine's ledger, a peer's inventory — had no way into world
//! memory at all, and a `[perception.printer_poll]` block written on the
//! assumption that it did sat in a live config for two weeks, parsed and
//! discarded.
//!
//! This is the generic half, and it is deliberately small: call one tool on one
//! server on a cadence, flatten the JSON it returns into `{name}.{path}` facts,
//! observe only what changed. No gate (these are not frames of people), no
//! rules (write a `[[reflex.rules]]` against the entities it produces), no
//! schema (the tool's output *is* the schema). What it shares with the ClawCam
//! poll is the seam: the result lands in the same store, with `Origin::Observed`
//! and a `source` that names the server, so the world-state block the model
//! reads every turn can say "printer.state = printing" with the same standing
//! as "power.mode = nominal".

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;

use crate::config::GenericPollConfig;
use crate::mcp::client::McpClient;
use crate::memory::world::{Origin, WorldMemory};

/// What one tick did. `changed` are the entities observed; `unchanged` were
/// skipped because the current belief already held that value; `truncated` are
/// leaves dropped past `max_facts` — counted, never silent.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IngestOutcome {
    pub changed: Vec<String>,
    pub unchanged: usize,
    pub truncated: usize,
}

/// A JSON key as an entity path segment: lowercase, `[a-z0-9_-]`, anything else
/// becomes `_`. Empty stays legible as `_` rather than producing `a..b`.
pub fn slug(key: &str) -> String {
    let out: String = key
        .trim()
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        "_".to_string()
    } else {
        out
    }
}

/// Flatten a tool result into `(entity, value)` leaves under `prefix`.
///
/// Objects recurse into dotted paths; arrays, scalars and empty objects are
/// leaves and keep their whole value. An array of runs is one fact — the thing
/// a reflex or the model wants to know is "what did the ledger say", not forty
/// facts named by index that reshuffle every tick.
pub fn flatten(prefix: &str, value: &Value, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (k, v) in map {
                flatten(&format!("{prefix}.{}", slug(k)), v, out);
            }
        }
        _ => out.push((prefix.to_string(), value.clone())),
    }
}

/// MCP tools return text. Most of the ones worth polling return JSON text; the
/// rest are recorded verbatim as one string fact rather than refused.
pub fn parse_tool_output(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.trim().to_string()))
}

/// Fold one parsed tool result into world memory. Only leaves whose value differs
/// from the current belief are observed, so a status polled every five seconds
/// does not write a row every five seconds; when a value moves, supersession
/// closes the old fact exactly as it does for any other observation.
pub fn ingest_poll_result(
    world: &WorldMemory,
    name: &str,
    parsed: &Value,
    now_ms: u64,
    source: &str,
    max_facts: usize,
) -> anyhow::Result<IngestOutcome> {
    let mut flat = Vec::new();
    flatten(name, parsed, &mut flat);
    let truncated = flat.len().saturating_sub(max_facts);
    flat.truncate(max_facts);

    let mut outcome = IngestOutcome {
        truncated,
        ..Default::default()
    };
    for (entity, value) in flat {
        let unchanged = world
            .current(&entity)?
            .map(|f| f.value == value)
            .unwrap_or(false);
        if unchanged {
            outcome.unchanged += 1;
            continue;
        }
        // This is the boundary where the content enters the system, off a tool
        // call to an external server, so origin is decided here: a reading,
        // not a framework rollup. `observe()` would default to `Derived`.
        world.observe_as(&entity, value, now_ms, now_ms, source, Origin::Observed)?;
        outcome.changed.push(entity);
    }
    Ok(outcome)
}

/// One tick: call the configured tool over the shared client and ingest. The
/// client lock is released before the (synchronous) ingest runs.
pub async fn poll_once(
    client: Arc<Mutex<McpClient>>,
    world: &WorldMemory,
    cfg: &GenericPollConfig,
    now_ms: u64,
) -> anyhow::Result<IngestOutcome> {
    let raw = {
        let mut guard = client.lock().await;
        guard.call_tool(&cfg.tool, cfg.args.clone()).await?
    };
    let parsed = parse_tool_output(&raw);
    ingest_poll_result(
        world,
        &cfg.name,
        &parsed,
        now_ms,
        &cfg.source_label(),
        cfg.max_facts,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entities(out: &[(String, Value)]) -> Vec<&str> {
        out.iter().map(|(e, _)| e.as_str()).collect()
    }

    #[test]
    fn objects_recurse_and_everything_else_is_a_leaf() {
        let v = json!({
            "state": "printing",
            "temps": { "Nozzle": 210.5, "bed": 60 },
            "queue": [ {"id": 1}, {"id": 2} ],
            "empty": {}
        });
        let mut out = Vec::new();
        flatten("printer", &v, &mut out);
        let mut got = entities(&out);
        got.sort();
        assert_eq!(
            got,
            vec![
                "printer.empty",
                "printer.queue",
                "printer.state",
                "printer.temps.bed",
                "printer.temps.nozzle",
            ]
        );
        let queue = out.iter().find(|(e, _)| e == "printer.queue").unwrap();
        assert!(
            queue.1.is_array(),
            "arrays stay whole: one fact, not one per index"
        );
    }

    #[test]
    fn a_scalar_or_text_result_is_one_fact_at_the_prefix() {
        let mut out = Vec::new();
        flatten("odc", &parse_tool_output("not json at all"), &mut out);
        assert_eq!(out, vec![("odc".to_string(), json!("not json at all"))]);
    }

    #[test]
    fn slugs_are_entity_safe() {
        assert_eq!(slug("Nozzle Temp (C)"), "nozzle_temp__c_");
        assert_eq!(slug("  "), "_");
        assert_eq!(slug("ok-key_1"), "ok-key_1");
    }

    #[test]
    fn only_changes_are_observed_and_supersession_closes_the_old_value() {
        let world = WorldMemory::open_in_memory().unwrap();
        let first = json!({"state": "idle", "temps": {"nozzle": 25}});
        let o1 = ingest_poll_result(&world, "printer", &first, 1_000, "mcp:printer", 64).unwrap();
        assert_eq!(o1.changed.len(), 2);
        assert_eq!(o1.unchanged, 0);

        // Same reading five seconds later: nothing is written.
        let o2 = ingest_poll_result(&world, "printer", &first, 6_000, "mcp:printer", 64).unwrap();
        assert!(o2.changed.is_empty());
        assert_eq!(o2.unchanged, 2);
        assert_eq!(world.history("printer.state").unwrap().len(), 1);

        // One value moves: exactly that entity gets a new fact, the old one closes.
        let second = json!({"state": "printing", "temps": {"nozzle": 25}});
        let o3 = ingest_poll_result(&world, "printer", &second, 11_000, "mcp:printer", 64).unwrap();
        assert_eq!(o3.changed, vec!["printer.state".to_string()]);
        assert_eq!(o3.unchanged, 1);
        let hist = world.history("printer.state").unwrap();
        assert_eq!(hist.len(), 2);
        let current = world.current("printer.state").unwrap().unwrap();
        assert_eq!(current.value, json!("printing"));
        assert_eq!(current.source, "mcp:printer");
        assert_eq!(
            current.origin,
            Origin::Observed,
            "a poll result is a reading"
        );
    }

    #[test]
    fn the_leaf_cap_drops_and_counts_rather_than_failing() {
        let world = WorldMemory::open_in_memory().unwrap();
        let mut wide = serde_json::Map::new();
        for i in 0..10 {
            wide.insert(format!("k{i}"), json!(i));
        }
        let o = ingest_poll_result(&world, "wide", &Value::Object(wide), 1, "mcp:wide", 4).unwrap();
        assert_eq!(o.changed.len(), 4);
        assert_eq!(o.truncated, 6);
    }
}
