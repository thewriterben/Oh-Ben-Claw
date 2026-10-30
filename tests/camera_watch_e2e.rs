//! `camera-watch`, end to end, with the board replaced by something that
//! speaks its protocol.
//!
//! The pieces are the real ones: `Esp32Link` over a stream, the firmware's
//! line format, the bench fixture's JPEGs, `CameraHealth` writing world
//! memory, and the camera reflex reading it. Only the board is fake: an
//! in-memory node that answers `camera_capture` with recorded frames, chatters
//! between replies the way the real one does, and then stops being able to
//! capture.
//!
//! What this does not cover is everything a board adds: USB-Serial-JTAG
//! timing, the open-reset, a ring that fills. `Esp32Link::open_serial` and
//! the 3.5 s settle are untested here and want one bench run.

use base64::Engine as _;
use oh_ben_claw::agent::reflex::{Action, ReflexEngine};
use oh_ben_claw::memory::world::WorldMemory;
use oh_ben_claw::peripherals::esp32_link::Esp32Link;
use oh_ben_claw::vision::camera_health::{camera_health_rules, CameraHealth, CameraMode};
use oh_ben_claw::vision::camera_percepts::CameraPercepts;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader};

fn person_frames() -> Vec<Vec<u8>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vision-bench-2026-09-16/person");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "jpg"))
        .collect();
    files.sort();
    files.iter().map(|p| std::fs::read(p).unwrap()).collect()
}

/// Serves `frames` in order, one per `camera_capture`, then answers every
/// later capture with the firmware's failure shape.
async fn fake_node(stream: tokio::io::DuplexStream, frames: Vec<Vec<u8>>) {
    let (r, mut w) = tokio::io::split(stream);
    let mut lines = BufReader::new(r).lines();
    let mut served = 0usize;
    while let Ok(Some(line)) = lines.next_line().await {
        let req: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(req["cmd"], "camera_capture");
        let id = req["id"].as_str().unwrap();
        // The node reports on its own between replies; the link must skip it.
        w.write_all(b"{\"type\":\"beacon\",\"node_id\":\"obc-esp32-s3-003\"}\n")
            .await
            .unwrap();
        let reply = match frames.get(served) {
            Some(jpeg) => serde_json::json!({
                "id": id,
                "ok": true,
                "result": base64::engine::general_purpose::STANDARD.encode(jpeg),
            }),
            None => serde_json::json!({
                "id": id, "ok": false, "result": "", "error": "capture: fb_get timed out",
            }),
        };
        served += 1;
        w.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
    }
}

#[tokio::test]
async fn frames_become_health_facts_and_a_dead_camera_escalates() {
    let frames = person_frames();
    let n_frames = frames.len();
    let (host, node) = duplex(256 * 1024);
    tokio::spawn(fake_node(node, frames));

    let world = Arc::new(WorldMemory::open_in_memory().unwrap());
    let health = CameraHealth::new("lilygo").with_world_memory(Arc::clone(&world));
    let engine = ReflexEngine::new(camera_health_rules("lilygo", 0));
    let mut percepts = CameraPercepts::new("lilygo");
    let mut perceived = Vec::new();
    let mut link = Esp32Link::new(host);

    // The loop `run_camera_watch` runs, minus the port and the sleep.
    let mut modes = Vec::new();
    let mut events = 0;
    let mut escalations = Vec::new();
    for i in 0..(n_frames + 3) as u64 {
        let capture = link.capture_jpeg(Duration::from_secs(5)).await.unwrap();
        let v = health.observe_capture(capture, i * 800).unwrap();
        events += usize::from(v.motion_event);
        modes.push(v.mode);
        escalations.extend(engine.tick(&world, i * 800).unwrap());
        perceived.extend(percepts.tick(&world).unwrap().into_iter().map(|p| p.text));
    }

    // The recorded frames: warm-up first, then the person set's 20 motion
    // pairs (the same count the fixture tests hold the classifier to).
    assert_eq!(&modes[..2], &[CameraMode::Warming, CameraMode::Warming]);
    assert_eq!(events, 20);

    // Then the camera stops capturing: two failures are the bench's normal,
    // the third is `failed`, and the reflex escalates once.
    assert_eq!(
        &modes[n_frames..],
        &[CameraMode::Ok, CameraMode::Ok, CameraMode::Failed]
    );
    let fact = world.current("camera.lilygo").unwrap().unwrap();
    assert_eq!(fact.value["mode"], "failed");
    assert_eq!(fact.value["failure"], "failed: capture: fb_get timed out");
    assert_eq!(escalations.len(), 1, "{escalations:?}");
    assert!(
        matches!(&escalations[0].action, Action::Escalate { reason } if reason.contains("lilygo"))
    );

    // And what the mushroom body would be handed: not 20 motion frames but
    // one episode (they are 0.8 s apart), then the camera dying.
    assert_eq!(
        perceived
            .iter()
            .filter(|t| t.starts_with("motion at camera lilygo"))
            .count(),
        1,
        "{perceived:?}"
    );
    assert_eq!(perceived.last().unwrap(), "camera lilygo stopped capturing");
    assert!(perceived.len() <= 8, "events, not frames: {perceived:?}");
}
