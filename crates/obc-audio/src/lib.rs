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
//!
//! ## Why this is a crate
//!
//! Extracted 2026-08-13, and it took two struct fields to make it possible.
//! `SpineSpeechSink` held an `Arc<SpineClient>` and `TtsSpeechSink` held a
//! `TextToSpeechTool`; between them they were this module's only edges into the
//! agent tree, and the `audio <-> tools` cycle was the second of them. Both are
//! now implemented on the far side of the trait they satisfy —
//! `crate::spine::speech` and `crate::tools::builtin::audio_speech` upstream.
//!
//! What stayed is what belongs: the vocabulary ([`suite::HeardEvent`],
//! [`suite::Utterance`]), the [`suite::SpeechSink`] trait, the one sink that
//! depends on nothing ([`suite::LoggingSpeechSink`], which is what makes it the
//! safe default), and the controller that records both directions into world
//! memory.

pub mod suite;
