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
/// Human-in-the-loop approval — autonomy levels, the per-call gate, forever
/// grants, and the trust score a tool's output carries. Extracted to
/// [`obc_approval`] on 2026-08-13.
///
/// It reached zero blocking edges the same day, when `AutonomyLevel` and
/// `AutonomyConfig` moved here from the root config module. They had spent an
/// hour in `agent` first, which kept a cycle alive; autonomy level is the
/// approval policy, so this is where it belongs.
pub use obc_approval as approval;
/// Hearing and speaking — heard events and utterances recorded into world
/// memory, behind a pluggable [`obc_audio::suite::SpeechSink`]. Extracted to
/// [`obc_audio`] on 2026-08-13.
///
/// It was blocked by two struct fields, one per sink with a dependency. Both
/// sinks now live on the far side of the trait they implement:
/// `obc_spine::speech` and `obc_tools::builtin::audio_speech`.
///
/// Thirteen import sites in seven modules name `obc_audio` directly; this alias
/// is for `src/main.rs`.
pub use obc_audio as audio;
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
/// Multi-node coordination — the node registry, the cost-based task auction and
/// frontier exploration assignment. Extracted to [`obc_fleet`] on 2026-08-13.
///
/// It was blocked by two `use` lines: the spine bridge at the bottom of the
/// module, which is now `obc_spine::fleet_bridge` where the transport is.
/// The coordinator never needed to know MQTT exists.
///
/// Six import sites in three modules name `obc_fleet` directly; this alias is
/// for `src/main.rs` and `tests/`.
pub use obc_fleet as fleet;
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
/// Self-authored reflexes — rule synthesis from experience, extracted to
/// [`obc_learning`] on 2026-08-13.
///
/// It was blocked by `foresight`, which was blocked by `reflex`, which was
/// blocked by one action sink holding an `Arc<SpineClient>`. Three crates came
/// out from behind one field.
///
/// The invariant worth knowing before reading it: a mined proposal is never
/// activated. It is inert until approved, and `tests/learning_approval_gate.rs`
/// asserts that from the outside against a real `ForesightEngine`.
pub use obc_learning as learning;
pub mod mcp;
// The memory substrate lives in its own crate (2026-07-30). Re-exported under the
// old name so all twenty-three consumers keep compiling against `crate::memory::…`
// — a crate split that renamed every call site would be two changes at once, and
// only one of them is the point.
pub use obc_memory as memory;
/// Declarative multi-step missions — a sequence of steps with guards, run
/// against world memory and the navigation controller. Extracted to
/// [`obc_mission`] on 2026-08-13.
///
/// It reached zero blocking edges when `audio` became a crate the same day, and
/// this is the first extraction that reduces `config`'s outward edge count:
/// the root `Config` composes a `Vec<Mission>`, and that edge is now free.
pub use obc_mission as mission;
/// Typed, safety-bounded actuation — the act side of the
/// perceive→remember→reflex→act loop, extracted to [`obc_movement`] on
/// 2026-08-08.
///
/// It was blocked for months by exactly one edge: `Arc<obc_spine::SpineClient>`
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
pub use obc_planner;
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
/// The communication spine — an alias for the [`obc_spine`] crate, which has
/// held the MQTT backbone, the LoRa gateway and mesh, the mesh supervisor, the
/// P2P transport and the four sinks since 2026-08-14.
///
/// 5100 lines, and for most of this project's history the largest thing in the
/// tree that could not move. It reached zero blocking edges without any of its
/// own code changing: the four things holding it were sinks and bridges that
/// belonged on this side of the boundary anyway, and each one arrived here from
/// the module that used to own it — `SpineActuatorSink` from `movement`,
/// `SpineActionSink` from the agent's reflex module, the fleet bridge from the
/// coordinator, `SpineSpeechSink` from the audio suite. Trait where the
/// abstraction is, implementation where the dependency is, four times over.
///
/// What it unblocks is the point. `tools` — 8880 lines, the largest module
/// left — was blocked by exactly one edge, and that edge was this one.
pub use obc_spine as spine;
/// The tool layer — an alias for the [`obc_tools`] crate, which has held the
/// registry and every built-in tool since 2026-08-14.
///
/// 8880 lines across 29 files: the largest module in this tree, and the one the
/// rest of the endgame was queued behind. It was blocked by exactly one edge.
/// `spine` left the same day and this reached zero within the hour.
///
/// Not one of its 63 outward references pointed at a module still in this tree
/// — they went to memory, security, geo, movement, sensing, siteplan,
/// navigation, power, aerial, comms, gnss and audio, every one of them already
/// a crate. By line count this looked like the hardest thing left for months.
/// It was the loosest.
pub use obc_tools as tools;
// Network tunnels left for `obc-tunnel` on 2026-08-06, same day and same shape
// as obc-cost: its only edge outside itself was `TunnelConfig`, its own config
// block in the root config module, so the struct went with the providers that
// read it. `crate::tunnel::…` and `crate::config::TunnelConfig` are unchanged.
pub use obc_tunnel as tunnel;
pub mod vision;
pub use config::Config;
