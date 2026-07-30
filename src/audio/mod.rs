//! Audio.
//!
//! The live audio path is [`suite`] — the `hear` tool and the audio suite
//! controller — together with `audio_transcribe` and `text_to_speech` in
//! `tools::builtin::audio`.
//!
//! ## What used to be here
//!
//! An `AudioPipeline` (microphone → speech-to-text → agent → text-to-speech) and
//! an `AudioPipelineTool` to trigger it, 650 lines with eight public types.
//! Removed 2026-07-30: nothing outside this file referenced any of them. The
//! audio suite covers the same ground and is the path that runs; the pipeline was
//! superseded and left in place, and `ROADMAP.md` carried it as complete.
//!
//! It is recoverable from git history if the staged
//! microphone-to-speaker shape is wanted back — but it would be wanted back as a
//! wiring decision, not as a restoration, because it was never wired.

pub mod suite;
