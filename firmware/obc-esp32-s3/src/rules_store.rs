//! Host-pushed reflex rules across a reboot.
//!
//! Rules arrive over USB (`set_reflex_rules`) and, until 2026-09-13, lived only
//! in RAM. Every reset — crash, brown-out, a nudged cable, the bench's own
//! RTS pulse — returned the node to its six built-in safing rules and nothing
//! else, silently. The brain's posture then arrived over the mesh and moved a
//! slot that no rule was bound to (world memory, 2026-09-13 12:52–13:27: 154
//! reflex reports, every one `safe-link-offline`, the die-temperature rules
//! gone since a power cycle nobody had noticed).
//!
//! This is the node-side half of the answer; the host-side half (limits
//! re-pushed on a boot announcement, `mesh_supervisor::hydrate_limits`) came
//! first. The two are deliberately different:
//!
//! - **Limits stay RAM-only and deny-all at boot.** They are actuator
//!   *authority*, and the 2026-08-22 decision that a reset must never widen
//!   policy stands. The host holds them and re-pushes them; they fit a mesh
//!   frame.
//! - **Rules persist here.** They carry no authority — every `gpio_write` a
//!   rule fires still passes the Track 0 gate, which is deny-all until the
//!   host says otherwise — and they do *not* fit a mesh frame (a single rule
//!   is 330 bytes against 228), so the host cannot re-push them to a node in
//!   the field. A rule set that survives its own node's reboot is System 1
//!   keeping its own promise: "a node keeps reacting when the host is
//!   unreachable" includes when it has just rebooted.
//!
//! The record is tagged with the firmware version and a schema number. A
//! stored set from another firmware is not loaded — the wire form may have
//! changed under it — and is cleared so the mismatch is reported once, not
//! at every boot. A stored set that fails the same validation a push gets is
//! treated the same way. Whatever happened is said in the boot announcement
//! (`policy_state.rules`), so the host can tell "restored" from "started
//! empty" without asking.
//!
//! Pure (`std` + `serde`), so it tests on the host under
//! `tests/firmware_node_gates.rs`; the NVS behind [`Store`] lives in `main.rs`.

use crate::reflex::ReflexRule;
use serde::{Deserialize, Serialize};

/// NVS namespace and key the record lives under.
pub const NAMESPACE: &str = "reflex";
pub const KEY: &str = "rules";

/// Bumped when the stored form changes in a way a decode cannot absorb.
pub const SCHEMA: u32 = 1;

/// The largest record written. A rule set arrives on a serial line of at most
/// 2048 bytes (`MAX_LINE_LEN`), so this bounds the record with room for the
/// envelope; NVS blobs go far larger, but a record this size is already a
/// rule set no host ever pushed.
pub const MAX_BYTES: usize = 3072;

/// What is written: the rules and the firmware that understood them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub schema: u32,
    pub firmware_version: String,
    pub rules: Vec<ReflexRule>,
}

/// Where the record lives. The NVS implementation is in `main.rs`; tests use a
/// `Vec<u8>` in memory.
pub trait Store {
    /// The stored bytes, or `None` if nothing has ever been written (or the
    /// store is unreadable — the caller cannot tell those apart, and treats
    /// both as "start empty").
    fn load(&mut self) -> Option<Vec<u8>>;
    /// Replace the record.
    fn save(&mut self, bytes: &[u8]) -> Result<(), String>;
    /// Remove the record.
    fn clear(&mut self) -> Result<(), String>;
}

/// What a boot found.
#[derive(Debug, Clone, PartialEq)]
pub enum Loaded {
    /// A record from this firmware, valid: these host rules are in force again.
    Rules(Vec<ReflexRule>),
    /// Nothing stored: the node starts with its built-in safing rules only.
    None,
    /// A record from another firmware (or schema). Not loaded; cleared.
    Stale {
        firmware_version: String,
        schema: u32,
    },
    /// A record this firmware could not parse or would refuse at a push. Not
    /// loaded; cleared. The string says why.
    Corrupt(String),
}

impl Loaded {
    /// The word the boot announcement carries.
    pub fn source(&self) -> &'static str {
        match self {
            Loaded::Rules(_) => "nvs",
            Loaded::None => "none",
            Loaded::Stale { .. } => "stale",
            Loaded::Corrupt(_) => "corrupt",
        }
    }

    /// How many host rules came back.
    pub fn count(&self) -> usize {
        match self {
            Loaded::Rules(r) => r.len(),
            _ => 0,
        }
    }
}

/// Serialise the host rules for storage. Refuses a record over [`MAX_BYTES`]
/// rather than letting NVS decide.
pub fn encode(firmware_version: &str, rules: &[ReflexRule]) -> Result<Vec<u8>, String> {
    let record = Record {
        schema: SCHEMA,
        firmware_version: firmware_version.to_string(),
        rules: rules.to_vec(),
    };
    let bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err(format!(
            "rule record is {} bytes; the store keeps at most {MAX_BYTES}",
            bytes.len()
        ));
    }
    Ok(bytes)
}

/// Judge stored bytes against this firmware. Pure; [`boot`] applies the verdict
/// to the store.
pub fn decode(bytes: Option<&[u8]>, firmware_version: &str) -> Loaded {
    let Some(bytes) = bytes else {
        return Loaded::None;
    };
    let record: Record = match serde_json::from_slice(bytes) {
        Ok(r) => r,
        Err(e) => return Loaded::Corrupt(format!("record does not parse: {e}")),
    };
    if record.schema != SCHEMA || record.firmware_version != firmware_version {
        return Loaded::Stale {
            firmware_version: record.firmware_version,
            schema: record.schema,
        };
    }
    // The same door a push goes through. A rule the node would refuse live is
    // not one it should wake up running.
    for r in &record.rules {
        if let Err(e) = r.when.validate() {
            return Loaded::Corrupt(format!("rule {}: {e}", r.id));
        }
    }
    Loaded::Rules(record.rules)
}

/// Read the store at boot and clear anything that will not be loaded, so a
/// stale or corrupt record is announced at this boot and gone by the next.
pub fn boot(store: &mut impl Store, firmware_version: &str) -> Loaded {
    let bytes = store.load();
    let loaded = decode(bytes.as_deref(), firmware_version);
    if matches!(loaded, Loaded::Stale { .. } | Loaded::Corrupt(_)) {
        let _ = store.clear();
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflex::{Action, Cmp, Condition};

    #[derive(Default)]
    struct Mem {
        bytes: Option<Vec<u8>>,
        saves: usize,
        clears: usize,
    }

    impl Store for Mem {
        fn load(&mut self) -> Option<Vec<u8>> {
            self.bytes.clone()
        }
        fn save(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.saves += 1;
            self.bytes = Some(bytes.to_vec());
            Ok(())
        }
        fn clear(&mut self) -> Result<(), String> {
            self.clears += 1;
            self.bytes = None;
            Ok(())
        }
    }

    fn die_rule(id: &str, op: Cmp, value: i64) -> ReflexRule {
        ReflexRule {
            id: id.into(),
            when: Condition::SensorSlot {
                entity: "sensor.die_temperature".into(),
                op,
                slot: 0,
                min: 30.0,
                max: 70.0,
                default: 0.5,
            },
            then: Action::GpioWrite {
                node_id: "self".into(),
                pin: 21,
                value,
            },
            debounce_ms: 10_000,
            max_rate_hz: None,
            fire_on_change: true,
            hold_ms: 3_000,
        }
    }

    fn die_rules() -> Vec<ReflexRule> {
        vec![
            die_rule("die-hot", Cmp::Gt, 0),
            die_rule("die-cool", Cmp::Le, 1),
        ]
    }

    #[test]
    fn the_rules_a_host_pushed_are_the_rules_the_next_boot_runs() {
        let mut store = Mem::default();
        store.save(&encode("0.1.0", &die_rules()).unwrap()).unwrap();
        // Reboot: the same bytes, judged by the same firmware.
        match boot(&mut store, "0.1.0") {
            Loaded::Rules(r) => assert_eq!(r, die_rules(), "byte-identical round trip"),
            other => panic!("{other:?}"),
        }
        assert_eq!(store.clears, 0);
    }

    #[test]
    fn nothing_stored_starts_empty_and_says_so() {
        let mut store = Mem::default();
        let l = boot(&mut store, "0.1.0");
        assert_eq!(l, Loaded::None);
        assert_eq!(l.source(), "none");
        assert_eq!(l.count(), 0);
    }

    #[test]
    fn another_firmwares_rules_are_not_loaded_and_are_cleared_once() {
        // The wire form may have changed under the record; the built-in safing
        // rules are what this firmware knows it can run. Reported at this
        // boot, gone by the next, so the mismatch is news once.
        let mut store = Mem::default();
        store.save(&encode("0.0.9", &die_rules()).unwrap()).unwrap();
        let l = boot(&mut store, "0.1.0");
        assert_eq!(
            l,
            Loaded::Stale {
                firmware_version: "0.0.9".into(),
                schema: SCHEMA
            }
        );
        assert_eq!(store.clears, 1);
        assert_eq!(
            boot(&mut store, "0.1.0"),
            Loaded::None,
            "cleared: the next boot starts empty"
        );
    }

    #[test]
    fn a_record_a_push_would_refuse_is_refused_at_boot_too() {
        // Same door: a slot the node cannot hold fails `validate` on a push and
        // fails it here. Stored bytes are not a way around the check.
        let mut bad = die_rules();
        if let Condition::SensorSlot { slot, .. } = &mut bad[0].when {
            *slot = 16;
        }
        let mut store = Mem::default();
        store.save(&encode("0.1.0", &bad).unwrap()).unwrap();
        match boot(&mut store, "0.1.0") {
            Loaded::Corrupt(why) => {
                assert!(why.contains("die-hot") && why.contains("slot 16"), "{why}")
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(store.clears, 1);
    }

    #[test]
    fn bytes_that_are_not_a_record_are_corrupt_not_a_crash() {
        let mut store = Mem {
            bytes: Some(b"{\"schema\":1,\"rules\":".to_vec()),
            ..Default::default()
        };
        assert!(matches!(boot(&mut store, "0.1.0"), Loaded::Corrupt(_)));
        assert_eq!(store.clears, 1);
    }

    #[test]
    fn an_empty_host_set_is_a_record_too() {
        // "The host pushed no rules" and "nothing was ever pushed" are the same
        // node state, and both load as no host rules.
        let mut store = Mem::default();
        store.save(&encode("0.1.0", &[]).unwrap()).unwrap();
        assert_eq!(boot(&mut store, "0.1.0"), Loaded::Rules(vec![]));
    }

    #[test]
    fn a_record_too_large_for_the_store_is_refused_before_it_is_written() {
        let many: Vec<ReflexRule> = (0..20)
            .map(|i| die_rule(&format!("rule-{i}"), Cmp::Gt, 0))
            .collect();
        let err = encode("0.1.0", &many).unwrap_err();
        assert!(err.contains("bytes"), "{err}");
    }

    #[test]
    fn the_die_rules_fit_with_room() {
        let bytes = encode("0.1.0", &die_rules()).unwrap();
        assert!(bytes.len() < MAX_BYTES / 2, "{} bytes", bytes.len());
    }
}
