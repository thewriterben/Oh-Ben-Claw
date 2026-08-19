//! Built-in tools for the Oh-Ben-Claw agent.

pub mod aerial;
pub mod audio;
/// The tool layer's `SpeechSink` — utterances rendered to files via the TTS
/// tool. Moved here from `audio::suite` on 2026-08-13.
pub mod audio_speech;
pub mod audio_suite;
pub mod browser;
pub mod comms;
pub mod file;
pub mod fleet;
pub mod foresight;
pub mod gnss;
pub mod http;
pub mod incident;
pub mod learn;
pub mod memory;
pub mod mesh;
pub mod mission;
pub mod movement;
pub mod navigation;
pub mod ota;
pub mod power;
pub mod sensing;
pub mod shell;
pub mod site_anchor;
pub mod siteplan;
pub mod vision;
pub mod world;
