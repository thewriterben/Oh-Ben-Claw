//! Oh-Ben-Claw — Advanced multi-device AI assistant.
//!
//! This crate provides the core library for the Oh-Ben-Claw system.
//! It extends the ZeroClaw architecture with a distributed, multi-device
//! coordination layer built on MQTT.
//!
//! # Architecture
//!
//! The system is organized around three layers:
//!
//! - **Brain** (`agent`): The central LLM-powered reasoning engine.
//! - **Spine** (`spine`): The MQTT-based communication backbone.
//! - **Appendages** (`peripherals`): Firmware and drivers for hardware nodes.
//!
//! # Feature Flags
//!
//! - `hardware`: Enable USB device discovery and serial port communication.
//! - `mqtt-spine`: Enable the MQTT communication spine.
//! - `peripheral-rpi`: Enable Raspberry Pi GPIO via rppal (Linux only).
//! - `peripheral-nanopi`: Enable NanoPi Neo3 GPIO via sysfs (Linux only).
//! - `gui`: Enable the native GUI application.

// NOTE: this crate deliberately does NOT carry a blanket
// `#![allow(dead_code, unused_imports, unused_variables)]`. It used to, and that
// is how five unreachable modules and a documented-but-never-called
// PersonalityStore sat here without a single warning. Suppress narrowly, at the
// item, with a reason — never at the crate root.
//
// Public library API — items are exported for use by external consumers (CLI, GUI,
// tests, and future integrations). A library legitimately exposes more surface than
// its own binary uses; that is an argument for `pub`, not for silencing the lint.

/// Agent-to-agent protocol, extracted to [`obc_a2a`] on 2026-08-08 and served
/// over HTTP by `oh-ben-claw a2a-serve`.
///
/// This comment used to read "No `src/` consumer, but `tests/evals.rs` pins its
/// wire shape as a release gate — which is exactly the kind of consumer a
/// source-only survey misses. Kept." All true, and it undersold the problem.
/// Measured before the extraction: `src/main.rs` named this module zero times,
/// `src/gateway/` zero times, and with `pub` removed so `dead_code` could see
/// it, rustc reported 34 items never constructed. 791 conformant, tested lines
/// implementing a server nobody could start.
///
/// It was the cleanest extraction candidate in the crate — zero edges to
/// anything else here — and that was the same fact seen from the other side.
/// So it was wired first and extracted second, in that order and deliberately:
/// a public crate nobody can call is worse in public than in private, because
/// there it reads as a feature.
///
/// `crate::a2a::…` and `oh_ben_claw::a2a::…` are unchanged.
pub use obc_a2a as a2a;
/// The [`a2a::TaskExecutor`] implementation that dispatches to the real agent.
///
/// Deliberately not inside `obc-a2a`: that crate names nothing in this one,
/// which is the property that let it become a crate at all. Trait there,
/// implementation here — one file's worth of inconvenience for a boundary that
/// already existed.
pub mod a2a_agent;
// `aerial` and `gnss` left for `obc-position` on 2026-08-06 — one crate, because
// they are one pattern: a real-world position report projected through a site
// frame into the `NodeState` the fleet coordinates on. They became extractable
// the day before, when `NodeState` moved out of `fleet`; that one 25-line struct
// was the only thing pinning a 177-line and a 336-line module to an 823-line
// coordinator.
//
// Re-exported under the old names, so `crate::aerial::…` and `crate::gnss::…`
// are unchanged at every call site.
pub use obc_position::{aerial, gnss};
pub mod agent;
pub mod approval;
pub mod audio;
pub mod channels;
// The three body-telemetry suites live in their own crate (2026-08-01), the
// first piece picked by `scripts/extractability.py` rather than by hand. They
// are one crate because they are one pattern: reading -> classified world-memory
// fact -> derived mode a reflex watches. Re-exported under the old names, so
// `crate::comms::…`, `crate::power::…` and `crate::sensing::…` are unchanged at
// every call site.
pub use obc_telemetry::comms;
pub mod config;
/// The persistent sink for conscience decisions, moved into [`obc_conscience`]
/// where the record format, the replay reader and the config fingerprint it
/// stamps already live. It was the only piece of that story outside the crate,
/// which meant the public crate defined a log format it could not write.
///
/// Not an extraction — 163 lines do not want their own crate. `obc-conscience`
/// gains `anyhow` and `std::fs` by taking it; that is the whole cost, and it
/// buys a crate that can produce the evidence its own doc comment demands.
/// `oh_ben_claw::decision_log::…` and `crate::decision_log::…` are unchanged.
pub use obc_conscience::decision_log;
// Token accounting and the spending budget left for `obc-cost` on 2026-08-06.
// Its only edge outside itself was `CostConfig`, its own config block sitting in
// the root config module — so the config struct went with it, the way
// `DeploymentConfig` sits in obc-planner and `ConscienceConfig` in
// obc-conscience. `crate::cost::…` and `crate::config::CostConfig` are both
// unchanged.
pub use obc_cost as cost;
pub mod deployment;
pub mod doctor;
pub mod fleet;
/// Track 1 — the predictive control layer: trend forecasts over bitemporal
/// world memory, and rules that fire on a *predicted* threshold crossing
/// rather than a present one. Extracted to [`obc_foresight`] on 2026-08-13.
///
/// It was blocked by `reflex` alone, and went to zero blocking edges when that
/// became a crate the day before — the extraction moved no logic, only seven
/// `crate::memory::world::` paths that had been a crate since July.
///
/// As with [`reflex`](crate::agent::reflex), this alias is not a compatibility
/// shim: the four import sites in `config`, `learning`, `tools` and `vision`
/// name `obc_foresight` directly. What is left behind it is `src/main.rs`,
/// which is the binary, and `tests/`, which exercises the public surface on
/// purpose.
pub use obc_foresight as foresight;
pub mod gateway;
/// Local-tangent-plane geometry and site models — re-exported from the
/// [`obc_planner`] crate, where it sits next to the site optimizer that consumes
/// it. `oh_ben_claw::geo::X` is unchanged.
pub use obc_planner::geo;
pub mod harness;
pub mod learning;
pub mod mcp;
// The memory substrate lives in its own crate (2026-07-30). Re-exported under the
// old name so all twenty-three consumers keep compiling against `crate::memory::…`
// — a crate split that renamed every call site would be two changes at once, and
// only one of them is the point.
pub use obc_memory as memory;
pub use obc_planner;
pub mod mission;
/// Typed, safety-bounded actuation — the act side of the
/// perceive→remember→reflex→act loop, extracted to [`obc_movement`] on
/// 2026-08-08.
///
/// It was blocked for months by exactly one edge: `Arc<crate::spine::SpineClient>`
/// in one struct field and one constructor parameter of the single
/// `ActuatorSink` implementation that talked to the spine. Two lines of type
/// against a 741-line module. That sink now lives in [`spine::actuator`] and
/// implements this crate's trait from there, which is the same direction
/// `obc_a2a::TaskExecutor` and `RiskClass` were turned.
///
/// `crate::movement::…` is unchanged at every call site.
pub use obc_movement as movement;
/// Monte Carlo localization, pose-graph SLAM, occupancy and cost maps, A*
/// planning, frontier exploration and pose fusion — extracted to
/// [`obc_navigation`] on 2026-08-08, the same day as [`obc_movement`].
///
/// 3714 lines across nine files, and nothing in it was refactored to make the
/// move possible. Its one blocking edge was `movement`, whose one blocking edge
/// was a single `Arc<SpineClient>` field. Turning that edge took 39 lines;
/// this crate — six times the size of the one it was waiting on — followed with
/// no further design work. Measure the edges, not the line count.
///
/// `crate::navigation::…` is unchanged at every call site.
pub use obc_navigation as navigation;
// The agent's self-instrumentation lives in its own crate (2026-08-02) — spans,
// the span ring buffer, and the counters `/api/v1/metrics` serves. Re-exported
// under the old name so `crate::observability::…` is unchanged at every call
// site in reflex, system2, orchestrator, skill_forge and the gateway.
//
// `obc_telemetry` is the agent watching its body; this is the agent watching
// itself. The two were named apart on purpose and are easy to confuse.
pub use obc_observability as observability;
pub mod peripherals;
pub use obc_telemetry::power;
pub mod providers;
// Cron, interval and one-shot tasks live in their own crate (2026-08-02) — the
// last module the extractability survey listed with zero blocking edges.
// Re-exported under the old name so `crate::scheduler::…` is unchanged in
// main.rs and the four gateway routes that drive it.
/// The site coverage optimizer — re-exported from the [`obc_planner`] crate.
pub use obc_planner::siteplan;
/// Track 0 — an alias for the [`obc_safety`] crate, which has held the
/// implementation since 2026-07-30.
///
/// This was `pub mod security;` over a twelve-line `src/security.rs` whose
/// entire body was `pub use obc_safety::*;`. Identical to a reader and to the
/// compiler, and not identical to a survey: every tool that measures module
/// edges counted `crate::security::…` as a dependency on a module still in
/// this tree. It is 44 crossings from the core — 27% of everything
/// `scripts/core_endgame.py` measured — pointing at a crate that left months
/// ago.
///
/// The map said the core was more tangled than it is, because a file existed
/// whose only content was a redirect. `crate::security::…` is unchanged.
pub use obc_safety as security;
pub use obc_scheduler as scheduler;
pub use obc_telemetry::sensing;
pub mod skill_forge;
pub mod spine;
pub mod tools;
// Network tunnels left for `obc-tunnel` on 2026-08-06, same day and same shape
// as obc-cost: its only edge outside itself was `TunnelConfig`, its own config
// block in the root config module, so the struct went with the providers that
// read it. `crate::tunnel::…` and `crate::config::TunnelConfig` are unchanged.
pub use obc_tunnel as tunnel;
pub mod vision;
pub use config::Config;
