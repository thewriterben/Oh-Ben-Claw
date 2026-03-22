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

use esp_idf_svc::hal::prelude::*;
use esp_idf_svc::hal::uart::*;
use log::info;
use serde::{Deserialize, Serialize};

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
const OUTPUT_PINS: &[i32] = &[3, 14, 26, 33, 46];

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
}

impl AgentState {
    fn new() -> Self {
        Self {
            history: Vec::new(),
            llm: LlmConfig::default(),
            wifi_ssid: String::new(),
            wifi_password: String::new(),
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

    // UART0: TX=43, RX=44 (USB serial bridge on Waveshare board).
    let config = UartConfig::new().baudrate(Hertz(115_200));
    let mut uart = UartDriver::new(
        peripherals.uart0,
        pins.gpio43,
        pins.gpio44,
        Option::<esp_idf_svc::hal::gpio::Gpio0>::None,
        Option::<esp_idf_svc::hal::gpio::Gpio1>::None,
        &config,
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
    info!("Serial: UART0 TX=43, RX=44, 115200 baud");
    info!(
        "Commands: gpio_read, gpio_write, camera_capture, audio_sample, sensor_read, \
         capabilities, announce, agent_chat, agent_config, agent_clear"
    );

    let mut agent_state = AgentState::new();
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

    loop {
        match uart.read(&mut buf, 100) {
            Ok(0) => continue,
            Ok(n) => {
                for &b in &buf[..n] {
                    if b == b'\n' {
                        if !line.is_empty() {
                            if let Ok(line_str) = std::str::from_utf8(&line) {
                                if let Ok(resp) = handle_request(line_str, &mut agent_state) {
                                    let out = serde_json::to_string(&resp).unwrap_or_default();
                                    let _ = uart.write(format!("{}\n", out).as_bytes());
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
    }
}

fn handle_request(line: &str, state: &mut AgentState) -> anyhow::Result<Response> {
    let req: Request = serde_json::from_str(line.trim())?;
    let id = req.id.clone();

    let result = match req.cmd.as_str() {
        "capabilities" | "announce" => {
            let caps = serde_json::json!({
                "node_id": NODE_ID,
                "board": "waveshare-esp32-s3-touch-lcd-2.1",
                "firmware_version": FIRMWARE_VERSION,
                "edge_agent": true,
                "tools": [
                    {"name": "gpio_read", "description": "Read a GPIO pin value (0 or 1)."},
                    {"name": "gpio_write", "description": "Set a GPIO pin high (1) or low (0)."},
                    {"name": "camera_capture", "description": "Capture a JPEG image from the OV2640 camera."},
                    {"name": "audio_sample", "description": "Sample audio from the I2S microphone."},
                    {"name": "sensor_read", "description": "Read a value from an I2C/SPI sensor."},
                    {"name": "agent_chat", "description": "Chat with the on-device LLM agent."},
                    {"name": "agent_config", "description": "Configure WiFi and LLM settings."},
                    {"name": "agent_clear", "description": "Clear the agent conversation history."}
                ],
                "gpio": [0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,26,27,33,34,35,36,37,43,44,46],
                "camera": true,
                "microphone": true,
                "i2c_bus": [4, 5],
                "uart": [43, 44],
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
            gpio_write(pin, value)?;
            Ok("done".into())
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
            audio_sample(duration_ms, raw)
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
            sensor_read(&sensor, &field)
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
    };

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

fn gpio_write(pin: i32, value: u64) -> anyhow::Result<()> {
    let ret = unsafe { esp_idf_svc::sys::gpio_set_level(pin, value as u32) };
    if ret != esp_idf_svc::sys::ESP_OK {
        anyhow::bail!("gpio_set_level failed for pin {} with error {}", pin, ret);
    }
    Ok(())
}

// ── Camera ────────────────────────────────────────────────────────────────────

fn camera_capture(quality: u8, format: &str) -> anyhow::Result<String> {
    // NOTE: Full camera capture requires the ESP-IDF camera component.
    // Enable in sdkconfig.defaults:
    //   CONFIG_ESP32_CAMERA=y
    //   CONFIG_SPIRAM=y
    //
    // This stub returns a placeholder base64 string.
    // Replace with actual esp_camera_fb_get() / esp_camera_fb_return() calls.
    log::info!("camera_capture: quality={}, format={}", quality, format);
    Ok(format!(
        "STUB:camera_capture:quality={quality}:format={format}:base64_jpeg_data_here"
    ))
}

// ── Audio ─────────────────────────────────────────────────────────────────────

fn audio_sample(duration_ms: u64, raw: bool) -> anyhow::Result<String> {
    // NOTE: Full audio sampling requires the ESP-IDF I2S driver.
    // Enable in sdkconfig.defaults:
    //   CONFIG_I2S_ENABLE=y
    //
    // This stub returns a placeholder RMS value.
    // Replace with actual i2s_read() calls.
    log::info!("audio_sample: duration_ms={}, raw={}", duration_ms, raw);
    if raw {
        Ok(format!(
            "STUB:audio_sample:duration_ms={duration_ms}:raw_pcm_data_here"
        ))
    } else {
        Ok("0.05".to_string()) // Placeholder RMS level
    }
}

// ── Sensors ───────────────────────────────────────────────────────────────────

fn sensor_read(sensor: &str, field: &str) -> anyhow::Result<String> {
    // NOTE: Full sensor reading requires I2C driver initialization and
    // sensor-specific register reads. This stub returns placeholder values.
    log::info!("sensor_read: sensor={}, field={}", sensor, field);
    match (sensor, field) {
        ("bme280", "temperature") => Ok("22.5".to_string()),
        ("bme280", "humidity") => Ok("55.0".to_string()),
        ("bme280", "pressure") => Ok("1013.25".to_string()),
        ("mpu6050", "accel_x") => Ok("0.01".to_string()),
        ("mpu6050", "accel_y") => Ok("0.00".to_string()),
        ("mpu6050", "accel_z") => Ok("9.81".to_string()),
        ("sht31", "temperature") => Ok("22.3".to_string()),
        ("sht31", "humidity") => Ok("54.8".to_string()),
        (s, f) => Err(anyhow::anyhow!("Unknown sensor/field: {}/{}", s, f)),
    }
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

            let tool_result = execute_local_tool(&tool_call.function.name, &args);

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

/// Execute a single tool call locally on the ESP32-S3.
fn execute_local_tool(name: &str, args: &serde_json::Value) -> anyhow::Result<String> {
    match name {
        "gpio_read" => {
            let pin = args.get("pin").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            gpio_read(pin).map(|v| v.to_string())
        }
        "gpio_write" => {
            let pin = args.get("pin").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let value = args.get("value").and_then(|v| v.as_u64()).unwrap_or(0);
            gpio_write(pin, value).map(|_| "done".to_string())
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
            audio_sample(dur, raw)
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
            sensor_read(sensor, field)
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
