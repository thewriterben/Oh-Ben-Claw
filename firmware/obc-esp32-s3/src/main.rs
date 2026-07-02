//! Oh-Ben-Claw ESP32-S3 firmware.
//!
//! This firmware runs on the Waveshare ESP32-S3 Touch LCD 2.1 (and compatible
//! ESP32-S3 boards) and exposes the following capabilities to the Oh-Ben-Claw
//! core agent:
//!
//! - **GPIO**: Read and write digital GPIO pins.
//! - **Camera**: Capture JPEG images from an OV2640 camera module.
//! - **Audio**: Sample audio from an I2S microphone (INMP441 / SPH0645).
//! - **Sensors**: Read I2C/SPI sensors (BME280, MPU6050, SHT31, BMP180).
//!
//! # Communication
//!
//! The firmware supports two communication modes:
//!
//! 1. **Serial (JSON-over-UART)**: The host sends newline-delimited JSON
//!    commands to UART0 (TX=GPIO43, RX=GPIO44, 115200 baud), and the board
//!    responds with newline-delimited JSON results. This is the same protocol
//!    as the base ZeroClaw ESP32 firmware.
//!
//! 2. **MQTT**: The firmware connects to a WiFi network and an MQTT broker,
//!    announces its capabilities, and receives tool call requests over the
//!    Oh-Ben-Claw MQTT spine. This enables network-based, multi-device
//!    coordination without a direct USB connection.
//!
//! # Edge-Native Mode (Phase 7)
//!
//! When the `agent_chat` command is received the firmware acts as an
//! **independent agent** — it connects to a cloud LLM API over WiFi, runs a
//! minimal agent loop, and responds without needing a host machine:
//!
//! ```json
//! {"id":"1","cmd":"agent_chat","args":{"message":"What is the temperature?"}}
//! ```
//!
//! The agent loop:
//! 1. Appends the user message to an in-memory ring-buffer history.
//! 2. Sends the history to a cloud LLM via HTTPS (OpenAI-compatible API).
//! 3. If the LLM requests a local tool call (gpio_read, sensor_read, …),
//!    executes it and feeds the result back.
//! 4. Returns the final assistant response.
//!
//! WiFi credentials and the LLM API key are read from NVS at boot.
//! Use `agent_config` to write them:
//!
//! ```json
//! {"id":"2","cmd":"agent_config","args":{
//!     "wifi_ssid":"MyNetwork",
//!     "wifi_password":"secret",
//!     "llm_api_key":"sk-...",
//!     "llm_base_url":"https://api.openai.com",
//!     "llm_model":"gpt-4o-mini"
//! }}
//! ```
//!
//! # Wiring (Waveshare ESP32-S3 Touch LCD 2.1)
//!
//! ## Camera (OV2640 via FPC connector)
//! | Signal | GPIO |
//! |--------|------|
//! | XCLK   | 15   |
//! | SIOD   | 4    |
//! | SIOC   | 5    |
//! | D0–D7  | 39–42, 16–19 |
//! | VSYNC  | 21   |
//! | HREF   | 38   |
//! | PCLK   | 13   |
//!
//! ## I2S Microphone (INMP441 / SPH0645)
//! | Signal | GPIO |
//! |--------|------|
//! | SCK    | 0    |
//! | WS     | 1    |
//! | SD     | 2    |
//!
//! ## I2C Sensor Bus (BME280, MPU6050, SHT31, etc.)
//! | Signal | GPIO |
//! |--------|------|
//! | SDA    | 4    |
//! | SCL    | 5    |
//!
//! # Build & Flash
//!
//! ```bash
//! # Install ESP toolchain
//! cargo install espup && espup install && source ~/export-esp.sh
//!
//! # Build and flash
//! cd firmware/obc-esp32-s3
//! cargo build --release
//! cargo espflash flash --monitor
//! ```

// esp-idf-hal 0.46 removed the `prelude` module — import the specific items used.
use esp_idf_svc::hal::peripherals::Peripherals;
// Command I/O runs over the native USB-Serial-JTAG (the XIAO ESP32-S3's only USB
// interface), not UART0 — UART0's GPIO43/44 aren't wired to the XIAO's USB port.
use esp_idf_svc::hal::usb_serial::{UsbSerialConfig, UsbSerialDriver};
use log::info;
use serde::{Deserialize, Serialize};

/// On-MCU reflex mirror (Phase 18, System 1 at the edge).
mod reflex;

/// On-MCU safing mirror (Phase 18) — built-in battery self-protection.
mod safing;

/// On-MCU Track 0 safety gate — deterministic, host-pushable actuator limits.
mod safety;

/// Real I2C sensor drivers (MAX17048 fuel gauge, MPU6050 IMU).
// Under `--features camera` the I2C bus is disabled (shared SCCB pins), so the
// sensor constructor/probe paths are intentionally unused in that build.
#[cfg_attr(feature = "camera", allow(dead_code))]
mod sensors;

/// I2S microphone driver (loudness/RMS).
mod audio;

/// OV2640 camera driver — opt-in via `--features camera` (see CAMERA.md).
#[cfg(feature = "camera")]
mod camera;

/// Maximum line length for incoming serial commands (bytes).
const MAX_LINE_LEN: usize = 512;

/// Firmware version — must match the host-side `CARGO_PKG_VERSION`.
const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Node ID — set this to a unique identifier for each board in your fleet.
/// In production, this should be read from NVS (non-volatile storage).
const NODE_ID: &str = "obc-esp32-s3-001";

/// JPEG quality range.
const CAMERA_QUALITY_MIN: u64 = 1;
const CAMERA_QUALITY_MAX: u64 = 10;
const CAMERA_QUALITY_DEFAULT: u64 = 5;

/// Audio sample duration range (ms).
const AUDIO_DURATION_MIN_MS: u64 = 10;
const AUDIO_DURATION_MAX_MS: u64 = 1000;
const AUDIO_DURATION_DEFAULT_MS: u64 = 100;

/// Maximum number of turns retained in the in-memory agent conversation history.
const AGENT_HISTORY_MAX: usize = 10;

/// Maximum LLM tool-call iterations per user message.
const AGENT_MAX_TOOL_ITERATIONS: usize = 3;

/// Maximum LLM HTTP response body size in bytes.  Caps heap usage on the ESP32-S3.
const MAX_LLM_RESPONSE_SIZE: usize = 8 * 1024;

/// GPIO pins configured as outputs during startup.
///
/// XIAO ESP32-S3 safe set: the onboard user LED (GPIO21, active-low — write 0 to
/// light it) plus exposed header pads (D2=GPIO3, D5=GPIO6, D8=GPIO7, D9=GPIO8).
/// Deliberately avoids GPIO26–37 (consumed by the XIAO's octal PSRAM) and the
/// I2C (GPIO4/5) and I2S (GPIO1/2) pins.
const OUTPUT_PINS: &[i32] = &[21, 3, 6, 7, 8];

// ── Agent State ───────────────────────────────────────────────────────────────

/// A single message in the in-memory conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

/// Configuration for the cloud LLM provider (persisted in NVS in production).
#[derive(Debug, Clone)]
struct LlmConfig {
    /// Base URL for the OpenAI-compatible API (e.g. "https://api.openai.com").
    base_url: String,
    /// API key.
    api_key: String,
    /// Model name (e.g. "gpt-4o-mini").
    model: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com".to_string(),
            api_key: String::new(),
            model: "gpt-4o-mini".to_string(),
        }
    }
}

/// Mutable runtime state for the edge agent (stored in a global static so it
/// persists across serial command invocations within the same boot session).
struct AgentState {
    /// Rolling conversation history (at most `AGENT_HISTORY_MAX` turns).
    history: Vec<ChatMessage>,
    /// Cloud LLM configuration (overridden by `agent_config` command).
    llm: LlmConfig,
    /// WiFi SSID for edge-mode connectivity.
    wifi_ssid: String,
    /// WiFi password.
    wifi_password: String,
    /// On-MCU reflex engine (System 1), populated from host-pushed rules.
    reflex: reflex::ReflexEngine,
    /// On-MCU Track 0 gate for actuator writes (default-deny; host-pushable).
    safety: safety::SafetyGate,
    /// Real I2C sensor bus, if one initialised at boot. `None` ⇒ sensor reads fall
    /// back to the stub, so the node still boots and reacts without sensors wired.
    sensors: Option<sensors::SensorBus>,
    /// I2S microphone, if one initialised at boot. `None` ⇒ `audio_sample` falls
    /// back to the stub RMS.
    audio: Option<audio::AudioMic>,
}

impl AgentState {
    fn new() -> Self {
        Self {
            history: Vec::new(),
            llm: LlmConfig::default(),
            wifi_ssid: String::new(),
            wifi_password: String::new(),
            reflex: reflex::ReflexEngine::default(),
            safety: safety::SafetyGate::with_output_pins(OUTPUT_PINS),
            sensors: None,
            audio: None,
        }
    }

    /// Append a message to history, evicting the oldest entry if needed.
    fn push_message(&mut self, role: &str, content: &str) {
        if self.history.len() >= AGENT_HISTORY_MAX {
            // Remove the oldest non-system message
            let drop_idx = self
                .history
                .iter()
                .position(|m| m.role != "system")
                .unwrap_or(0);
            self.history.remove(drop_idx);
        }
        self.history.push(ChatMessage {
            role: role.to_string(),
            content: content.to_string(),
        });
    }
}

// ── Wire Types ────────────────────────────────────────────────────────────────

/// Incoming command from the host (or from a local REPL/serial terminal).
#[derive(Debug, Deserialize)]
struct Request {
    id: String,
    cmd: String,
    /// Optional — commands with no arguments (e.g. `capabilities`) omit it, so it
    /// defaults to `Null` and the handlers fall back to their per-arg defaults.
    #[serde(default)]
    args: serde_json::Value,
}

/// Outgoing response to the host.
#[derive(Debug, Serialize)]
struct Response {
    id: String,
    ok: bool,
    result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

// ── OpenAI-Compatible HTTP Types ──────────────────────────────────────────────

/// A minimal chat-completion request body (OpenAI-compatible).
#[derive(Serialize)]
struct LlmRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    max_tokens: u32,
    temperature: f32,
}

/// A minimal tool call returned by the LLM.
#[derive(Debug, Deserialize)]
struct LlmToolCall {
    id: String,
    function: LlmFunctionCall,
}

#[derive(Debug, Deserialize)]
struct LlmFunctionCall {
    name: String,
    arguments: String,
}

/// A single choice from the LLM response.
#[derive(Debug, Deserialize)]
struct LlmChoice {
    message: LlmMessage,
}

#[derive(Debug, Deserialize)]
struct LlmMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<LlmToolCall>,
}

/// Top-level chat completion response.
#[derive(Debug, Deserialize)]
struct LlmResponse {
    choices: Vec<LlmChoice>,
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let pins = peripherals.pins;

    // Command channel over the native USB-Serial-JTAG (D-=GPIO19, D+=GPIO20 on the
    // ESP32-S3) — the interface the XIAO's USB-C port actually exposes. The host
    // sends newline-delimited JSON here and reads responses on the same connection.
    let mut usb = UsbSerialDriver::new(
        peripherals.usb_serial,
        pins.gpio19,
        pins.gpio20,
        &UsbSerialConfig::new(),
    )?;

    // Configure output pins via raw ESP-IDF sys API.
    unsafe {
        use esp_idf_svc::sys::*;
        for &pin in OUTPUT_PINS {
            gpio_reset_pin(pin);
            gpio_set_direction(pin, gpio_mode_t_GPIO_MODE_OUTPUT);
        }
    }

    info!("Oh-Ben-Claw ESP32-S3 firmware v{} ready", FIRMWARE_VERSION);
    info!("Node ID: {}", NODE_ID);
    info!("Serial: native USB-Serial-JTAG (send newline-delimited JSON commands)");
    info!(
        "Commands: gpio_read, gpio_write, camera_capture, audio_sample, sensor_read, \
         capabilities, announce, agent_chat, agent_config, agent_clear"
    );

    let mut agent_state = AgentState::new();
    // Real I2C sensor bus (SDA=GPIO4, SCL=GPIO5). If init fails (or no sensors are
    // fitted), reads fall back to the stub so the node still boots and the reflex
    // loop still runs.
    //
    // Disabled when the `camera` feature is on: the OV2640's SCCB uses these same
    // GPIO4/5 pins on this board, so the two can't share the bus. Wire the sensors
    // to other pins if you need both.
    #[cfg(not(feature = "camera"))]
    {
        use esp_idf_svc::hal::i2c::config::Config as I2cConfig;
        use esp_idf_svc::hal::i2c::I2cDriver;
        use esp_idf_svc::hal::units::FromValueType;
        let i2c_cfg = I2cConfig::new().baudrate(100_u32.kHz().into());
        match I2cDriver::new(peripherals.i2c0, pins.gpio4, pins.gpio5, &i2c_cfg) {
            Ok(drv) => {
                agent_state.sensors = Some(sensors::SensorBus::new(drv));
                info!("I2C sensor bus ready (SDA=4, SCL=5)");
            }
            Err(e) => {
                log::warn!("I2C sensor bus init failed ({e}); sensor reads fall back to stubs")
            }
        }
    }
    // OV2640 camera (opt-in). Owns the SCCB on GPIO4/5 and the parallel data bus.
    #[cfg(feature = "camera")]
    match camera::init() {
        Ok(()) => info!("OV2640 camera initialised"),
        Err(e) => log::warn!("camera init failed ({e}); camera_capture falls back to stub"),
    }
    // I2S microphone (SCK=GPIO0, WS=GPIO1, SD=GPIO2). Falls back to the stub RMS if
    // init fails or no mic is fitted.
    {
        use esp_idf_svc::hal::i2s::{config, I2sDriver};
        let i2s_cfg = config::StdConfig::philips(
            audio::SAMPLE_RATE_HZ,
            config::DataBitWidth::Bits32,
        );
        match I2sDriver::new_std_rx(
            peripherals.i2s0,
            &i2s_cfg,
            pins.gpio0,                                            // BCLK / SCK
            pins.gpio2,                                            // DIN / SD
            Option::<esp_idf_svc::hal::gpio::AnyIOPin>::None,      // no MCLK
            pins.gpio1,                                            // WS / LRCLK
        ) {
            Ok(drv) => {
                agent_state.audio = Some(audio::AudioMic::new(drv));
                info!("I2S mic ready (SCK=0, WS=1, SD=2)");
            }
            Err(e) => log::warn!("I2S mic init failed ({e}); audio_sample falls back to stub"),
        }
    }
    // Load the built-in safing rules so the node self-protects from boot, even
    // before (or without) any host-pushed rule set or spine connection.
    agent_state.reflex.set_rules(safing::default_safing_rules());
    log::info!("on-MCU safing rules loaded ({} built-in)", agent_state.reflex.rule_count());
    // System prompt prepended to every LLM request.
    agent_state.push_message(
        "system",
        "You are Oh-Ben-Claw running in edge-native mode on an ESP32-S3 microcontroller. \
         You have direct access to GPIO, camera, audio, and sensor tools on this device. \
         Keep responses short and precise — you are running on a resource-constrained device \
         with limited memory.",
    );

    let mut buf = [0u8; 512];
    let mut line: Vec<u8> = Vec::new();

    // Phase 18: System 1 at the edge — evaluate reflexes against on-board sensors
    // on a fixed cadence, independent of the host/spine. Rules arrive via the
    // `set_reflex_rules` command; fired GPIO actions are actuated locally through
    // the Track 0 safety gate and reported on `obc/nodes/{id}/reflex`.
    const REFLEX_INTERVAL_MS: u64 = 1000;
    let mut last_reflex_ms: u64 = 0;
    // Link watchdog: time of last host contact. If the host goes silent past the
    // safing timeout, the built-in `safe-link-offline` rule fires (on-MCU offline
    // safing), independent of battery safing.
    let mut last_host_contact_ms: u64 = now_ms();
    // Emit link/power status only when it *changes* (not every tick), so the serial
    // link isn't flooded with unchanged status — usable on a bench, and the right
    // behaviour for a real node reporting to a host.
    let mut last_link_offline: Option<bool> = None;
    let mut last_power_mode: Option<safing::PowerMode> = None;

    loop {
        // `Ok(0)` is a read timeout (no host data) — fall through to the reflex
        // tick rather than `continue`, so System 1 keeps running on its own.
        match usb.read(&mut buf, 100) {
            Ok(0) => {}
            Ok(n) => {
                last_host_contact_ms = now_ms(); // any host byte ⇒ link is alive
                for &b in &buf[..n] {
                    if b == b'\n' || b == b'\r' {
                        if !line.is_empty() {
                            if let Ok(line_str) = std::str::from_utf8(&line) {
                                if let Ok(resp) = handle_request(line_str, &mut agent_state) {
                                    let out = serde_json::to_string(&resp).unwrap_or_default();
                                    send_line(&mut usb, &out);
                                }
                            }
                            line.clear();
                        }
                    } else {
                        line.push(b);
                        if line.len() > MAX_LINE_LEN {
                            line.clear();
                        }
                    }
                }
            }
            Err(_) => {}
        }

        // ── Autonomous reflex tick (System 1) ─────────────────────────────────
        let now = now_ms();
        if now.saturating_sub(last_reflex_ms) >= REFLEX_INTERVAL_MS
            && agent_state.reflex.rule_count() > 0
        {
            last_reflex_ms = now;
            let mut snapshot = read_sensor_snapshot(&mut agent_state.sensors);
            // Feed the link-silence duration so the built-in `safe-link-offline`
            // rule can fire, and self-report the link state.
            let silence_ms = now.saturating_sub(last_host_contact_ms);
            snapshot.insert(safing::LINK_SILENCE_ENTITY.to_string(), silence_ms as f64);
            let link_offline = safing::link_offline(silence_ms as f64, safing::DEFAULT_LINK_TIMEOUT_MS as f64);
            // Report link state only on a change (online↔offline).
            if last_link_offline != Some(link_offline) {
                last_link_offline = Some(link_offline);
                let link_report = serde_json::json!({
                    "type": "link_state",
                    "node_id": NODE_ID,
                    "state": if link_offline { "offline" } else { "online" },
                    "silence_ms": silence_ms,
                    "ts_ms": now,
                });
                send_line(&mut usb, &link_report.to_string());
            }
            // Self-report the derived power mode when a battery reading is present —
            // only when the mode changes, so a steady battery is silent.
            if let Some(&soc) = snapshot.get(safing::BATTERY_SOC_ENTITY) {
                let mode = safing::derive(
                    soc,
                    false,
                    safing::DEFAULT_LOW_PCT,
                    safing::DEFAULT_CRITICAL_PCT,
                );
                if last_power_mode != Some(mode) {
                    last_power_mode = Some(mode);
                    let report = serde_json::json!({
                        "type": "power_mode",
                        "node_id": NODE_ID,
                        "mode": mode.as_str(),
                        "soc_pct": soc,
                        "ts_ms": now,
                    });
                    send_line(&mut usb, &report.to_string());
                }
            }
            for fired in agent_state.reflex.evaluate(&snapshot, now) {
                let mut applied = false;
                let mut error: Option<String> = None;
                if let reflex::Action::GpioWrite { pin, value, .. } = &fired.action {
                    // Safety-gated local actuation (Track 0).
                    match gpio_write(&mut agent_state.safety, *pin as i32, *value as u64, now) {
                        Ok(()) => applied = true,
                        Err(e) => error = Some(e.to_string()),
                    }
                }
                let report = serde_json::json!({
                    "type": "reflex",
                    "node_id": NODE_ID,
                    "rule_id": fired.rule_id,
                    "action": serde_json::to_value(&fired.action).unwrap_or(serde_json::Value::Null),
                    "applied": applied,
                    "error": error,
                    "ts_ms": now,
                });
                send_line(&mut usb, &report.to_string());
            }
        }
    }
}

fn handle_request(line: &str, state: &mut AgentState) -> anyhow::Result<Response> {
    let req: Request = serde_json::from_str(line.trim())?;
    let id = req.id.clone();

    // Wrap the dispatch in a closure so a `?` inside any arm returns *here* (into
    // `result`) rather than escaping `handle_request` — otherwise refused/errored
    // commands (e.g. a Track 0 safety denial) would send no reply at all.
    let result: anyhow::Result<String> = (|| {
        match req.cmd.as_str() {
        "capabilities" | "announce" => {
            let caps = serde_json::json!({
                "node_id": NODE_ID,
                "board": "seeed-xiao-esp32-s3",
                "firmware_version": FIRMWARE_VERSION,
                "edge_agent": true,
                "tools": [
                    {"name": "gpio_read", "description": "Read a GPIO pin value (0 or 1)."},
                    {"name": "gpio_write", "description": "Set a GPIO pin high (1) or low (0)."},
                    {"name": "camera_capture", "description": "Capture a JPEG image from the OV2640 camera."},
                    {"name": "audio_sample", "description": "Sample audio from the I2S microphone."},
                    {"name": "sensor_read", "description": "Read a value from an I2C/SPI sensor."},
                    {"name": "set_reflex_rules", "description": "Push the on-MCU reflex (System 1) rule set."},
                    {"name": "set_limits", "description": "Push the Track 0 actuator safety limits (allow-list, range, rate)."},
                    {"name": "agent_chat", "description": "Chat with the on-device LLM agent."},
                    {"name": "agent_config", "description": "Configure WiFi and LLM settings."},
                    {"name": "agent_clear", "description": "Clear the agent conversation history."}
                ],
                "gpio": [21, 3, 6, 7, 8],
                "camera": false,
                "microphone": true,
                "i2c_bus": [4, 5],
                "transport": "usb-serial-jtag",
                "wifi": true
            });
            Ok(caps.to_string())
        }

        "gpio_read" => {
            let pin = req.args.get("pin").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let value = gpio_read(pin)?;
            Ok(value.to_string())
        }

        "gpio_write" => {
            let pin = req.args.get("pin").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let value = req.args.get("value").and_then(|v| v.as_u64()).unwrap_or(0);
            gpio_write(&mut state.safety, pin, value, now_ms())?;
            Ok("done".into())
        }

        // Track 0: host pushes this node's deterministic actuator limits (mirror of
        // the host `[[safety.limit]]` set). Retained on `obc/nodes/{id}/limits`.
        // Tightens the boot default-deny policy in the field with no reflash.
        "set_limits" => {
            let limits: Vec<safety::SafetyLimit> = serde_json::from_value(
                req.args.get("limits").cloned().unwrap_or(serde_json::json!([])),
            )?;
            let applied = state.safety.apply_pushed(limits, NODE_ID);
            let policy = state.safety.policy();
            Ok(serde_json::json!({
                "applied": applied,
                "allowed_pins": policy.allowed_pins,
                "value_min": policy.value_min,
                "value_max": policy.value_max,
                "min_interval_ms": policy.min_interval_ms,
            })
            .to_string())
        }

        // Phase 18: host pushes this node's reflex rule set (mirror of the host
        // engine). Retained on `obc/nodes/{id}/reflex_rules` once the spine lands.
        "set_reflex_rules" => {
            let rules: Vec<reflex::ReflexRule> = serde_json::from_value(
                req.args.get("rules").cloned().unwrap_or(serde_json::json!([])),
            )?;
            let n = rules.len();
            // Keep the built-in safing rules in front of host-pushed rules so a
            // node never loses self-protection when the host replaces its set.
            let merged = safing::with_defaults(rules);
            let total = merged.len();
            state.reflex.set_rules(merged);
            Ok(serde_json::json!({ "loaded": n, "total": total, "builtin_safing": total - n }).to_string())
        }

        // Phase 18: evaluate reflexes against a sensor snapshot. Fired
        // `gpio_write` actions are actuated locally through the Track 0 safety
        // gate; the fired set is the `obc/nodes/{id}/reflex` report payload.
        "reflex_tick" => {
            let mut snapshot: std::collections::HashMap<String, f64> =
                std::collections::HashMap::new();
            if let Some(obj) = req.args.get("snapshot").and_then(|v| v.as_object()) {
                for (k, v) in obj {
                    if let Some(f) = v.as_f64() {
                        snapshot.insert(k.clone(), f);
                    }
                }
            }
            let now_ms = req.args.get("now_ms").and_then(|v| v.as_u64()).unwrap_or(0);
            let fired = state.reflex.evaluate(&snapshot, now_ms);

            let mut reports = Vec::with_capacity(fired.len());
            for f in &fired {
                let mut applied = false;
                let mut error: Option<String> = None;
                if let reflex::Action::GpioWrite { pin, value, .. } = &f.action {
                    match gpio_write(&mut state.safety, *pin as i32, *value as u64, now_ms) {
                        Ok(()) => applied = true,
                        Err(e) => error = Some(e.to_string()),
                    }
                }
                reports.push(serde_json::json!({
                    "rule_id": f.rule_id,
                    "action": serde_json::to_value(&f.action).unwrap_or(serde_json::Value::Null),
                    "applied": applied,
                    "error": error,
                }));
            }
            Ok(serde_json::json!({ "node_id": NODE_ID, "fired": reports }).to_string())
        }

        "camera_capture" => {
            let quality = req
                .args
                .get("quality")
                .and_then(|v| v.as_u64())
                .unwrap_or(CAMERA_QUALITY_DEFAULT)
                .clamp(CAMERA_QUALITY_MIN, CAMERA_QUALITY_MAX) as u8;
            let format = req
                .args
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("jpeg")
                .to_string();
            camera_capture(quality, &format)
        }

        "audio_sample" => {
            let duration_ms = req
                .args
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(AUDIO_DURATION_DEFAULT_MS)
                .clamp(AUDIO_DURATION_MIN_MS, AUDIO_DURATION_MAX_MS);
            let raw = req
                .args
                .get("raw")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            audio_sample(&mut state.audio, duration_ms, raw)
        }

        "sensor_read" => {
            let sensor = req
                .args
                .get("sensor")
                .and_then(|v| v.as_str())
                .unwrap_or("bme280")
                .to_string();
            let field = req
                .args
                .get("field")
                .and_then(|v| v.as_str())
                .unwrap_or("temperature")
                .to_string();
            read_sensor(&mut state.sensors, &sensor, &field).map(|v| v.to_string())
        }

        // ── Edge-Native Agent Commands ─────────────────────────────────────
        "agent_config" => {
            if let Some(ssid) = req.args.get("wifi_ssid").and_then(|v| v.as_str()) {
                state.wifi_ssid = ssid.to_string();
            }
            if let Some(pwd) = req.args.get("wifi_password").and_then(|v| v.as_str()) {
                state.wifi_password = pwd.to_string();
            }
            if let Some(key) = req.args.get("llm_api_key").and_then(|v| v.as_str()) {
                state.llm.api_key = key.to_string();
            }
            if let Some(url) = req.args.get("llm_base_url").and_then(|v| v.as_str()) {
                state.llm.base_url = url.to_string();
            }
            if let Some(model) = req.args.get("llm_model").and_then(|v| v.as_str()) {
                state.llm.model = model.to_string();
            }
            Ok("agent config updated".to_string())
        }

        "agent_clear" => {
            // Retain only the system message.
            state.history.retain(|m| m.role == "system");
            Ok("history cleared".to_string())
        }

        "agent_chat" => {
            let message = req
                .args
                .get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("agent_chat requires 'message' argument"))?
                .to_string();

            agent_chat(&message, state)
        }

        unknown => Err(anyhow::anyhow!("Unknown command: {}", unknown)),
        }
    })();

    match result {
        Ok(output) => Ok(Response {
            id,
            ok: true,
            result: output,
            error: None,
        }),
        Err(e) => Ok(Response {
            id,
            ok: false,
            result: String::new(),
            error: Some(e.to_string()),
        }),
    }
}

// ── GPIO ──────────────────────────────────────────────────────────────────────

fn gpio_read(pin: i32) -> anyhow::Result<u32> {
    let level = unsafe { esp_idf_svc::sys::gpio_get_level(pin) };
    Ok(level as u32)
}

/// Drive a GPIO pin, gated by the Track 0 [`safety::SafetyGate`].
///
/// Enforced on the MCU itself, so a compromised host, a poisoned skill, or a
/// hallucinated tool call still cannot drive a pin outside policy. The gate is
/// default-deny (boot policy = the `OUTPUT_PINS` allow-list, digital range 0..=1)
/// and can be tightened in the field by a host-pushed limit set (`set_limits`),
/// including a per-pin rate limit. `now_ms` is the monotonic clock the rate limit
/// measures against.
fn gpio_write(
    gate: &mut safety::SafetyGate,
    pin: i32,
    value: u64,
    now_ms: u64,
) -> anyhow::Result<()> {
    // Track 0: refuse out-of-policy actuator commands BEFORE touching hardware.
    gate.check(pin as i64, value as i64, now_ms)?;
    let ret = unsafe { esp_idf_svc::sys::gpio_set_level(pin, value as u32) };
    if ret != esp_idf_svc::sys::ESP_OK {
        anyhow::bail!("gpio_set_level failed for pin {} with error {}", pin, ret);
    }
    Ok(())
}

// ── Camera ────────────────────────────────────────────────────────────────────

fn camera_capture(quality: u8, format: &str) -> anyhow::Result<String> {
    #[cfg(feature = "camera")]
    {
        // Format/quality are baked into the driver config at init; capture returns
        // a base64 JPEG frame from the OV2640.
        let _ = (quality, format);
        camera::capture_base64()
    }
    #[cfg(not(feature = "camera"))]
    {
        // Built without the `camera` feature — return the placeholder (see CAMERA.md).
        log::info!("camera_capture stub: quality={}, format={}", quality, format);
        Ok(format!(
            "STUB:camera_capture:quality={quality}:format={format}:base64_jpeg_data_here"
        ))
    }
}

// ── Audio ─────────────────────────────────────────────────────────────────────

fn audio_sample(
    mic: &mut Option<audio::AudioMic>,
    duration_ms: u64,
    raw: bool,
) -> anyhow::Result<String> {
    if raw {
        // Streaming raw PCM over the serial link is impractical; keep the stub.
        return Ok(format!(
            "STUB:audio_sample:duration_ms={duration_ms}:raw_pcm_data_here"
        ));
    }
    if let Some(m) = mic.as_mut() {
        let level = m.rms(duration_ms)?;
        return Ok(format!("{level:.4}"));
    }
    Ok("0.05".to_string()) // stub RMS when no mic is fitted
}

// ── Sensors ───────────────────────────────────────────────────────────────────

/// Read a sensor value, preferring the real I2C bus and falling back to the stub
/// for sensors the driver does not (yet) handle or when no bus is present. Returns
/// a numeric value (host world-memory convention).
fn read_sensor(
    bus: &mut Option<sensors::SensorBus>,
    sensor: &str,
    field: &str,
) -> anyhow::Result<f64> {
    if let Some(b) = bus.as_mut() {
        if let Some(result) = b.read(sensor, field) {
            return result; // real reading (or an honest I2C error)
        }
    }
    sensor_read_stub(sensor, field)
}

/// Placeholder sensor values for parts the real driver does not cover yet (BME280
/// environment, SHT31) or when no I2C bus initialised. `max17048` is intentionally
/// absent so `sensor.battery_soc` stays out of the snapshot (safing dormant) unless
/// a real fuel gauge is present.
fn sensor_read_stub(sensor: &str, field: &str) -> anyhow::Result<f64> {
    match (sensor, field) {
        ("bme280", "temperature") => Ok(22.5),
        ("bme280", "humidity") => Ok(55.0),
        ("bme280", "pressure") => Ok(1013.25),
        ("mpu6050", "accel_x") => Ok(0.01),
        ("mpu6050", "accel_y") => Ok(0.00),
        ("mpu6050", "accel_z") => Ok(9.81),
        ("sht31", "temperature") => Ok(22.3),
        ("sht31", "humidity") => Ok(54.8),
        (s, f) => Err(anyhow::anyhow!("Unknown sensor/field: {}/{}", s, f)),
    }
}

// ── On-MCU reflex support (Phase 18, System 1) ────────────────────────────────

/// Write one newline-terminated line to the USB-Serial-JTAG, looping over partial
/// writes so a long response (e.g. `capabilities`) goes out whole. Each chunk uses
/// a short timeout, so if the host isn't reading the node doesn't block — it drops
/// the remainder and carries on (System 1 keeps ticking).
fn send_line(usb: &mut UsbSerialDriver, line: &str) {
    let payload = format!("{line}\n");
    let bytes = payload.as_bytes();
    let mut off = 0;
    let mut stalls = 0u32;
    while off < bytes.len() {
        match usb.write(&bytes[off..], 100) {
            Ok(0) => {
                // No progress this round (tx buffer full / host draining). Retry a
                // bounded number of times so large replies (e.g. `capabilities`)
                // get out, but give up after ~2 s if the host has truly gone.
                stalls += 1;
                if stalls > 20 {
                    return;
                }
            }
            Ok(n) => {
                off += n;
                stalls = 0;
            }
            Err(_) => return,
        }
    }
}

/// Monotonic milliseconds since boot (ESP timer), for reflex valid-time + debounce.
fn now_ms() -> u64 {
    (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1000) as u64
}

/// Build a reflex snapshot from the node's local sensors. Entity keys follow the
/// host world-memory convention (`sensor.{quantity}`) so a rule authored against
/// world memory (e.g. `sensor.temperature > 60`) evaluates identically on-device.
fn read_sensor_snapshot(
    bus: &mut Option<sensors::SensorBus>,
) -> std::collections::HashMap<String, f64> {
    const READS: &[(&str, &str, &str)] = &[
        ("sensor.temperature", "bme280", "temperature"),
        ("sensor.humidity", "bme280", "humidity"),
        ("sensor.pressure", "bme280", "pressure"),
        ("sensor.accel_x", "mpu6050", "accel_x"),
        ("sensor.accel_y", "mpu6050", "accel_y"),
        ("sensor.accel_z", "mpu6050", "accel_z"),
        // Battery state of charge from the fuel gauge — feeds the built-in safing
        // rules. Absent (read error / no gauge fitted) so safing stays dormant
        // rather than firing on a missing reading.
        ("sensor.battery_soc", "max17048", "soc"),
    ];
    let mut snapshot = std::collections::HashMap::new();
    for (entity, sensor, field) in READS {
        if let Ok(value) = read_sensor(bus, sensor, field) {
            snapshot.insert(entity.to_string(), value);
        }
    }
    snapshot
}

// ── Edge-Native Agent Loop ────────────────────────────────────────────────────

/// Execute the lightweight on-device agent loop for a single user message.
///
/// # Sequence
///
/// 1. Append the user message to the rolling history.
/// 2. Call the cloud LLM via HTTPS with the current history.
/// 3. If the LLM requests a tool call, execute it locally and repeat.
/// 4. Return the final assistant response.
///
/// WiFi must be connected before calling this function.  If the API key is
/// not configured, an error is returned immediately.
fn agent_chat(message: &str, state: &mut AgentState) -> anyhow::Result<String> {
    if state.llm.api_key.is_empty() {
        return Err(anyhow::anyhow!(
            "LLM API key not configured. Send: \
             {{\"id\":\"1\",\"cmd\":\"agent_config\",\"args\":{{\"llm_api_key\":\"sk-...\",...}}}}"
        ));
    }

    state.push_message("user", message);

    for _iteration in 0..AGENT_MAX_TOOL_ITERATIONS {
        let response = llm_chat_completion(state)?;

        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("LLM returned no choices"))?;

        // If no tool calls — we have the final answer.
        if choice.message.tool_calls.is_empty() {
            let text = choice.message.content.unwrap_or_default();
            state.push_message("assistant", &text);
            return Ok(text);
        }

        // Execute each tool call and feed results back.
        for tool_call in &choice.message.tool_calls {
            let args: serde_json::Value =
                serde_json::from_str(&tool_call.function.arguments).unwrap_or_default();

            let tool_result = execute_local_tool(
                &mut state.safety,
                &mut state.sensors,
                &mut state.audio,
                now_ms(),
                &tool_call.function.name,
                &args,
            );

            // Add tool result to history so the LLM can continue.
            let result_text = match tool_result {
                Ok(s) => s,
                Err(e) => e.to_string(),
            };
            let result_content = format!(
                "[Tool result for {} (id={})]: {}",
                tool_call.function.name, tool_call.id, result_text
            );
            state.push_message("user", &result_content);
        }
    }

    // Exhausted iterations — request a final summary.
    state.push_message(
        "user",
        "Please provide your final response based on the results above.",
    );
    let final_response = llm_chat_completion(state)?;
    let text = final_response
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .unwrap_or_else(|| "(no response)".to_string());
    state.push_message("assistant", &text);
    Ok(text)
}

/// Execute a single tool call locally on the ESP32-S3. `gpio_write` is gated by
/// the same Track 0 [`safety::SafetyGate`] as the host-command and reflex paths,
/// so an LLM-driven write cannot bypass the policy either.
fn execute_local_tool(
    gate: &mut safety::SafetyGate,
    sensors: &mut Option<sensors::SensorBus>,
    audio: &mut Option<audio::AudioMic>,
    now_ms: u64,
    name: &str,
    args: &serde_json::Value,
) -> anyhow::Result<String> {
    match name {
        "gpio_read" => {
            let pin = args.get("pin").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            gpio_read(pin).map(|v| v.to_string())
        }
        "gpio_write" => {
            let pin = args.get("pin").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let value = args.get("value").and_then(|v| v.as_u64()).unwrap_or(0);
            gpio_write(gate, pin, value, now_ms).map(|_| "done".to_string())
        }
        "camera_capture" => {
            let quality = args
                .get("quality")
                .and_then(|v| v.as_u64())
                .unwrap_or(CAMERA_QUALITY_DEFAULT)
                .clamp(CAMERA_QUALITY_MIN, CAMERA_QUALITY_MAX) as u8;
            let fmt = args
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("jpeg");
            camera_capture(quality, fmt)
        }
        "audio_sample" => {
            let dur = args
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(AUDIO_DURATION_DEFAULT_MS)
                .clamp(AUDIO_DURATION_MIN_MS, AUDIO_DURATION_MAX_MS);
            let raw = args.get("raw").and_then(|v| v.as_bool()).unwrap_or(false);
            audio_sample(audio, dur, raw)
        }
        "sensor_read" => {
            let sensor = args
                .get("sensor")
                .and_then(|v| v.as_str())
                .unwrap_or("bme280");
            let field = args
                .get("field")
                .and_then(|v| v.as_str())
                .unwrap_or("temperature");
            read_sensor(sensors, sensor, field).map(|v| v.to_string())
        }
        unknown => Err(anyhow::anyhow!("Unknown local tool: {}", unknown)),
    }
}

/// Send the current conversation history to the cloud LLM and return the
/// parsed response.
///
/// Uses the ESP-IDF embedded HTTP client (`esp_idf_svc::http::client`) for
/// the HTTPS request.
fn llm_chat_completion(state: &AgentState) -> anyhow::Result<LlmResponse> {
    use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};

    let url = format!(
        "{}/v1/chat/completions",
        state.llm.base_url.trim_end_matches('/')
    );

    let request_body = serde_json::to_string(&LlmRequest {
        model: &state.llm.model,
        messages: &state.history,
        max_tokens: 512,
        temperature: 0.7,
    })?;

    log::info!("LLM request to {} ({} bytes)", url, request_body.len());

    let http_config = HttpConfig {
        use_global_ca_store: true,
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };

    let mut client = EspHttpConnection::new(&http_config)?;

    let headers = [
        ("Content-Type", "application/json"),
        ("Authorization", &format!("Bearer {}", state.llm.api_key)),
    ];

    let request_bytes = request_body.as_bytes();
    client.initiate_request(esp_idf_svc::http::Method::Post, &url, &headers)?;
    client.write(request_bytes)?;
    client.initiate_response()?;

    let status = client.status();
    if status != 200 {
        anyhow::bail!("LLM API returned HTTP {}", status);
    }

    // Read the response body (cap at 8 KB to avoid heap exhaustion).
    let mut body = Vec::with_capacity(4096);
    let mut chunk = [0u8; 512];
    loop {
        match client.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                body.extend_from_slice(&chunk[..n]);
                if body.len() > MAX_LLM_RESPONSE_SIZE {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let response: LlmResponse = serde_json::from_slice(&body)
        .map_err(|e| anyhow::anyhow!("Failed to parse LLM response: {}", e))?;

    Ok(response)
}
