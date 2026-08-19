//! Body telemetry — battery, links and sensor streams, classified into world
//! memory as facts a reflex can act on.
//!
//! Three suites that are one pattern. Each ingests a raw reading, judges it
//! against configured expectations, writes it to bitemporal [`obc_memory`]
//! under a stable entity name, and derives a coarse mode that System 1 can
//! watch without understanding the domain:
//!
//! | suite | records | derived mode | what a reflex does with it |
//! |---|---|---|---|
//! | [`power`] | `power.battery` | `power.mode` — normal / low / critical / charging | stop motors, dim, sleep before the pack dies |
//! | [`comms`] | `link.{name}` | `net.mode` — best state across every link seen | buffer instead of stream, fall back to local System 1 |
//! | [`sensing`] | `sensor.{quantity}` with a quality flag | — | distrust an out-of-range or stale reading rather than acting on it |
//!
//! The derived mode is the part worth keeping. A reflex rule cannot reason
//! about millivolts or packet loss, and should not have to: the suite makes one
//! domain judgement, writes it as its own fact, and the rule watches that. It
//! is also what makes the three separable — nothing here knows about tools,
//! providers, the spine or the agent loop.
//!
//! This is the agent watching its **body**. The agent watching **itself** —
//! spans, approval counters, cost — is `obc-observability`, and the two are
//! unrelated despite the word.
//!
//! [`node`] is the fourth thing here and not a suite: [`node::NodeState`], the
//! heartbeat every other layer reads. It moved here from `fleet` on 2026-08-06
//! because it is telemetry a coordinator consumes, not coordination — and
//! because being defined inside `fleet` was the single edge keeping `aerial`
//! and `gnss` from being extractable at all.

pub mod comms;
pub mod node;
pub mod power;
pub mod sensing;

pub use node::NodeState;
