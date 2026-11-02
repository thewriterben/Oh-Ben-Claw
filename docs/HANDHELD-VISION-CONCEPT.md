# Handheld vision kit — concept

2026-11-01. **Concept only: nothing here is built, flashed or measured.** Numbers
marked *estimate* are arithmetic, not bench results.

## Goal

A portable handheld that identifies what it is looking at, and that is one member
of an OBC system rather than a closed gadget: modules and sensors add readings,
control and whatever else the hardware allows.

## What is already on the bench

| Part | What it brings | Source |
|---|---|---|
| LILYGO T-Deck (915 MHz) | ESP32-S3, SX1262 LoRa, 2.8" 320×240 touch LCD, keyboard, trackball, mic + speaker, microSD, battery connector. Grove UART on GPIO 43/44 is free on the base model. | `crates/obc-planner/src/peripherals/registry.rs` (T-Deck row), `docs/T-DECK-RESEARCH.md` |
| Emakefun / nulllaborg ESP32-S3-CAM | ESP32-S3R8, 8 MB OPI PSRAM, OV3660/OV2640, flash LED, TF card, LiPo charger on PH2.0, 38.4 × 30.4 mm | vendor PDF (esp32s3cam.pdf) |
| ESP32-S3-WROOM-1 CAM | 8 MB octal PSRAM, sold with OV5640, dual USB-C, microSD | vendor PDF (esp32s3cam2.pdf) |
| OV5640 DVP module | 5 MP, about 100 mm FPC, 100°–200° lens options | vendor PDF (cammod.pdf) |
| LILYGO T-CameraPlus-S3 V1.1 | camera, 1.3" TFT, PMIC: already a one-board handheld, with no keyboard or LoRa | `camera.rs`, registry |
| OBC host | `vision_analyze` sends an image to any OpenAI-compatible vision model (`OBC_VISION_MODEL`, can point at local Ollama). Routing keeps a turn local once this tool has run. | `crates/obc-tools/src/builtin/vision.rs`, `crates/obc-agent/src/routing.rs` |
| `firmware/t-deck-terminal` | T-Deck as LoRa spine console, gateway and relay. Reference firmware, **not yet bench-flashed**. | its README |

The ESP32-S3-CAM and WROOM CAM board profiles are in open PRs
(thewriterben/Oh-Ben-Claw#215, thewriterben/ClawCam#22), not yet merged or run on
metal.

## Shape: console, eye, brain

```
  [ eye ]  camera head: ESP32-S3-CAM + OV5640/OV3660, OBC camera build
     │  Grove UART (wired)  or  ESP-NOW / Wi-Fi (untethered)
  [ console ]  T-Deck: screen, keyboard, trackball, speaker, LoRa, battery
     │  Wi-Fi when in range (images)        LoRa 915 MHz spine (results only)
  [ brain ]  OBC host: vision_analyze → local vision model; fleet, memory, planner
```

- **Eye.** Captures the frame and runs an on-device first pass. QVGA (320×240),
  the size the OBC camera build already captures, fills the T-Deck screen exactly.
- **Console.** Shutter on the trackball press, frame plus label on screen, answer
  spoken through the speaker. Keyboard asks follow-ups ("is it edible?").
- **Brain.** Open-vocabulary identification. Results land in world memory like
  any other percept.

## Links, and why images do not go over LoRa

- **Eye ↔ console, wired:** Grove UART on GPIO 43/44, free on the base T-Deck
  (the Plus uses those pins for GNSS). A QVGA JPEG of about 10 KB at 921600 baud
  takes roughly 0.1 s (*estimate*). Wired is the v0 choice: no pairing, and the
  eye can be powered from the console.
- **Console ↔ brain:** images go over Wi-Fi. LoRa SF7 / BW125 moves about 5.5 kbps,
  so the same 10 KB JPEG needs around 15 s of airtime over about 40 frames of 255
  bytes (*estimate*). A result fits in one frame (label, confidence, position,
  time). **Off-grid, identification happens on the eye or not at all**; LoRa tells
  the fleet what was seen.

## Identification tiers

1. **On the eye, offline.** ESP-DL / TFLite Micro class models have fixed label
   sets (COCO-style objects, faces, people), run at low resolution, and manage a
   few frames per second at best (*estimate, measure it*). They work as the
   trigger and as the offline fallback.
   - The OBC camera build captures **greyscale** today; colour models need RGB565
     or JPEG capture, which is a camera config change.
2. **On the host.** A vision-language model through `vision_analyze`, answering
   in open vocabulary. Needs a link to the host.
3. **Not Accelerapp's camera AI.** `esp32_cam/ai_processing.py` `_run_inference`
   returns hard-coded detections (a placeholder at line 212). Accelerapp may still
   be useful for enclosure and cost generation (`hardware/design`, `economics`).

## Modules

- **Identity.** Every module is either an OBC node that self-reports through
  `capabilities` (`board::describe_json`), or a bus device a node reports for it.
  The planner already has the roles: Sensing, Peripheral, Localization,
  LongRangeRadio, Console, OperatorConsole (`deployment/inventory.rs`).
- **Physical bus.** Qwiic / STEMMA QT I2C on the eye. esp32-camera can share an
  already-open I2C port for SCCB.
  - **Address clash:** the OV5640 answers at 0x3C, the same address as common
    SSD1306 OLEDs.
- **Control** goes through Track 0 allow-lists, as everywhere in OBC. The new
  camera boards start with empty `output_pins` until each pin is cited.
- **Candidate first modules:**
  - ToF distance, to give the object's scale
  - IMU, to reject blurred frames and record orientation
  - GNSS (built into the T-Deck Plus, absent on the base model)
  - thermal array

## Phases

0. **Point and identify on the desk, no new hardware.** Existing camera build
   (XIAO Sense or T-CameraPlus today, ESP32-S3-CAM once its PR lands) → host
   `vision_analyze` → answer. Measure end-to-end latency.
1. **T-Deck as viewer.** Flash and verify `t-deck-terminal` first. Then add a
   vision page, the eye on Grove UART, a trackball shutter, and the label shown
   and spoken.
2. **Offline tier.** Detector on the eye; result frames on the LoRa spine.
3. **Modules and enclosure.**

## Open decisions

1. **Where identification lives:**
   - the host VLM, which needs Wi-Fi to answer open-ended questions;
   - fully on-device, with fixed labels and working anywhere;
   - Pi-class compute inside the handheld, at the cost of more power and weight.
2. **T-Deck base or Plus.** Only the base has the free Grove UART; only the Plus
   has GNSS and a built-in battery.
3. **Tethered or wireless eye.**
