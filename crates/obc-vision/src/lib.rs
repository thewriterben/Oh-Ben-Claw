//! Vision.
//!
//! The live vision path is the ClawCam suite below: ingest folds detections into
//! world memory, rules fire reflexes on them, analytics reads the aggregates and
//! actuate drives the camera.
//!
//! ## What used to be here
//!
//! A `VisionPipeline` (camera capture → analysis → structured result) with a
//! `VisionPipelineTool`, `CameraSource`, `VisionPipelineConfig`, `CapturedFrame`,
//! `VisionAnalysis` and a `mime_from_extension` helper — 540 lines, none of it
//! referenced outside this file. Removed 2026-07-30.
//!
//! It surfaced only after `src/multimodal.rs` was removed in the same pass, which
//! held the two calls to `mime_from_extension` that were keeping this file looking
//! half-alive. Dead code hides dead code, and a sweep is worth re-running after
//! every cut rather than once at the start.
//!
//! The registered vision tool is `vision_analyze` in `tools::builtin::vision`,
//! which is unaffected.
//!
//! [`motion`] is the frame-difference classifier measured on the camera bench
//! (`scripts/vision/classify.py`), ported so CI holds it to the fixture.
//! [`camera_health`] uses it to judge a camera's own state (warming, ok,
//! lighting shift, nudged, blind, failed) as a `camera.{node}` fact, fed by
//! `oh-ben-claw camera-watch`. [`jpeg`] decodes frames to the grey the
//! thresholds were measured on.

pub mod camera_health;
pub mod camera_percepts;
pub mod clawcam_actuate;
pub mod clawcam_analytics;
pub mod clawcam_ingest;
pub mod clawcam_rules;
pub mod clawcam_spatial;
pub mod jpeg;
pub mod motion;
