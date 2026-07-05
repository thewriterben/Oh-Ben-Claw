// t_deck_terminal — OBC handheld fleet console for the LilyGO T-Deck / T-Deck Plus
// =================================================================================
// Turns a T-Deck into an *interactive* member of the Oh-Ben-Claw LoRa spine:
//
//   1. CONSOLE  — the screen shows a live scrollback of every spine frame heard
//                 over the air (heartbeats, reflex/safing reports, chat), plus a
//                 status bar (battery, GPS fix, last RSSI). The QWERTY keyboard
//                 composes messages; the trackball scrolls the log.
//   2. GATEWAY  — when tethered over USB, every received frame is also printed to
//                 the host in the exact base-station console format the OBC host
//                 already parses (`src/spine/lora_gateway.rs`):
//                     SPINE ◄ src=AB seq=7 rssi=-42 dBm : {"type":...}
//                 and every newline-terminated JSON line the host writes (e.g. a
//                 `mesh_command` NodeCommand) is framed onto the mesh. A T-Deck on
//                 a USB cable is therefore a drop-in replacement for the Heltec
//                 base station — zero host-side changes.
//   3. RELAY    — frames with remaining TTL are flood-relayed onward with (src,seq)
//                 de-dup, exactly like `firmware/heltec-lora-linktest`.
//
// The human's outbound path mirrors the host's:
//   * plain text + Enter          → {"node_id":"tdeck-XX","type":"chat","text":...}
//   * /cmd <to> <cmd> [json-args] → {"id":"td-N","to":...,"cmd":...,"args":{...}}
//     — the NodeCommand line format from src/spine/lora_gateway.rs. Execution is
//     still gated ON THE TARGET NODE by its on-MCU Track 0 mirror; this console
//     has no authority of its own (a command it sends is treated exactly like one
//     from the host gateway path).
//   * /hb                          → toggle periodic heartbeats (position from GPS
//     on a T-Deck Plus, battery from the fuel ADC) so the console itself shows up
//     in world memory / the fleet Coordinator like any other node.
//
// ── On-air compatibility ────────────────────────────────────────────────────────
// Two OBC nets exist; pick with NET_MODE below (or toggle at runtime with /net):
//   NET_SPINE (default) — SpineFrame [src][seq][ttl][payload], SF7/BW125/CR4-5,
//     sync 0x12 (RadioLib writes SX126x regs 0x14,0x24 — matching the value
//     heltec-lora-linktest programs directly). Interoperates with the Heltec
//     gateway + obc-esp32-s3 nodes.
//   NET_FLEET — raw newline-less MeshFrame JSON on air ({"t":"hb",...}), SF10,
//     sync 0x2B. Interoperates with firmware/lora-node (obc_lora_bridge) and the
//     host's src/spine/lora_mesh.rs fleet codec.
//
// ── Hardware facts (verified against Xinyuan-LilyGO/T-Deck utilities.h and
//    T-DECK-RESEARCH.md, 2026-07) ─────────────────────────────────────────────────
//   * BOARD_POWERON GPIO10 must be HIGH before ANY peripheral responds.
//   * Display: ST7789 320x240 over the shared SPI bus (CS 12, DC 11, BL 42).
//   * Radio:   SX1262 on the same shared bus (CS 9, DIO1 45, RST 17, BUSY 13).
//   * SPI bus: SCK 40, MISO 38, MOSI 41 (display + radio + SD share it; we only
//     touch it from loop() context, never from an ISR).
//   * Keyboard: a separate ESP32-C3 as I2C slave @ 0x55 (SDA 18, SCL 8, INT 46).
//   * Trackball: 4x hall pulses — up 3, down 15, left 1, right 2.
//   * Battery ADC: GPIO 4. GPS (Plus only): UART TX 43 / RX 44 — u-blox M10Q SKUs
//     talk 38400 baud, Quectel L76K SKUs 9600; we autodetect.
//
// Libraries (Arduino Library Manager): RadioLib (Jan Gromes, 6.x),
// LovyanGFX (lovyan03, 1.1.x), TinyGPSPlus (Mikal Hart) — GPS optional, see
// HAS_GPS. Board settings: "ESP32S3 Dev Module", PSRAM: "OPI PSRAM",
// USB CDC On Boot: "Enabled", Flash Size: 16MB. To flash: hold the trackball
// center-press (BOOT) while powering on. NEVER transmit without an antenna.
//
// STATUS: reference firmware, written against documented APIs + the verified pin
// map, not yet bench-flashed. Validate radio params against your region before TX.

#include <RadioLib.h>
#include <SPI.h>
#include <Wire.h>
#include <esp_mac.h>  // esp_read_mac (present on ESP32 core 2.0.14 / IDF 4.4+)
#define LGFX_USE_V1
#include <LovyanGFX.hpp>

// ─── Build options ───────────────────────────────────────────────────────────────
#define HAS_GPS 1          // 1 = T-Deck Plus (or GPS shield on the base T-Deck)
#define NET_SPINE 0        // spine net: SpineFrame header, SF7, sync 0x12
#define NET_FLEET 1        // fleet net: raw MeshFrame JSON, SF10, sync 0x2B
static uint8_t netMode = NET_SPINE;   // startup net; /net toggles at runtime

static const float  RADIO_FREQ_MHZ  = 915.0;  // US ISM — match your region + host
static const int8_t RADIO_POWER_DBM = 17;     // respect regional EIRP limits
static const uint8_t SPINE_TTL      = 2;      // flood-relay hop budget (matches gw)
static const unsigned long HB_PERIOD_MS = 30000;

#if HAS_GPS
#include <TinyGPSPlus.h>
#endif

// ─── Pins (Xinyuan-LilyGO/T-Deck utilities.h) ────────────────────────────────────
static const int PIN_POWERON = 10;
static const int PIN_TFT_CS = 12, PIN_TFT_DC = 11, PIN_TFT_BL = 42;
static const int PIN_RADIO_CS = 9, PIN_RADIO_DIO1 = 45, PIN_RADIO_RST = 17, PIN_RADIO_BUSY = 13;
static const int PIN_SPI_SCK = 40, PIN_SPI_MISO = 38, PIN_SPI_MOSI = 41;
static const int PIN_SDCARD_CS = 39;  // held high so the SD card stays off the bus
static const int PIN_I2C_SDA = 18, PIN_I2C_SCL = 8;
static const int PIN_KB_INT = 46;
static const int PIN_TB_UP = 3, PIN_TB_DOWN = 15, PIN_TB_LEFT = 1, PIN_TB_RIGHT = 2;
static const int PIN_BAT_ADC = 4;
static const int PIN_GPS_TX = 43, PIN_GPS_RX = 44;  // ESP32 RX←GPS-TX on 44
static const uint8_t KB_I2C_ADDR = 0x55;

// ─── Display (LovyanGFX in-sketch panel config for the T-Deck ST7789) ────────────
class LGFX_TDeck : public lgfx::LGFX_Device {
  lgfx::Panel_ST7789 _panel;
  lgfx::Bus_SPI _bus;
  lgfx::Light_PWM _light;
 public:
  LGFX_TDeck() {
    { auto cfg = _bus.config();
      cfg.spi_host = SPI2_HOST;   // FSPI — same host the Arduino SPI object uses
      cfg.spi_mode = 0;
      cfg.freq_write = 40000000;
      cfg.freq_read  = 16000000;
      cfg.pin_sclk = PIN_SPI_SCK; cfg.pin_mosi = PIN_SPI_MOSI;
      cfg.pin_miso = PIN_SPI_MISO; cfg.pin_dc = PIN_TFT_DC;
      cfg.spi_3wire = false;
      cfg.use_lock = true;        // bus is shared with the SX1262 — keep the lock
      _bus.config(cfg); _panel.setBus(&_bus); }
    { auto cfg = _panel.config();
      cfg.pin_cs = PIN_TFT_CS; cfg.pin_rst = -1; cfg.pin_busy = -1;
      cfg.panel_width = 240; cfg.panel_height = 320;
      cfg.offset_rotation = 1;    // land on 320x240 landscape, keyboard below
      cfg.invert = true; cfg.rgb_order = false;
      cfg.bus_shared = true;
      _panel.config(cfg); }
    { auto cfg = _light.config();
      cfg.pin_bl = PIN_TFT_BL; cfg.invert = false; cfg.freq = 12000;
      cfg.pwm_channel = 7;
      _light.config(cfg); _panel.setLight(&_light); }
    setPanel(&_panel);
  }
};

LGFX_TDeck tft;
SX1262 radio = new Module(PIN_RADIO_CS, PIN_RADIO_DIO1, PIN_RADIO_RST, PIN_RADIO_BUSY);
#if HAS_GPS
TinyGPSPlus gps;
#endif

// ─── Spine frame + seen-set (mirrors firmware/heltec-lora-linktest/src/spine.rs) ──
static const size_t MAX_PAYLOAD = 240;
static const size_t SPINE_HEADER = 3;  // [src][seq][ttl]

struct Seen { uint8_t src, seq; };
static Seen seenRing[32];
static size_t seenHead = 0, seenLen = 0;
// Returns true if (src,seq) was already recorded; otherwise records it.
bool seenOrInsert(uint8_t src, uint8_t seq) {
  for (size_t i = 0; i < seenLen; i++) {
    size_t idx = (seenHead + 32 - 1 - i) % 32;
    if (seenRing[idx].src == src && seenRing[idx].seq == seq) return true;
  }
  seenRing[seenHead] = {src, seq};
  seenHead = (seenHead + 1) % 32;
  if (seenLen < 32) seenLen++;
  return false;
}

static uint8_t nodeId = 0;   // low byte of the STA MAC, like the other OBC firmware
static uint8_t txSeq = 0;
static uint32_t cmdSeq = 0;  // correlation-id counter for /cmd

// ─── RX interrupt plumbing (flag only; SPI reads stay in loop context) ───────────
volatile bool packetReady = false;
ICACHE_RAM_ATTR void onPacketReceived() { packetReady = true; }

// ─── UI state ────────────────────────────────────────────────────────────────────
static const int LOG_LINES = 100;          // scrollback ring
static String logRing[LOG_LINES];
static uint16_t logColor[LOG_LINES];
static int logHead = 0, logCount = 0, scrollBack = 0;
static String composeBuf;
static bool uiDirty = true, statusDirty = true;
static int lastRssi = 0;
static bool hbEnabled = true;
static unsigned long lastHbMs = 0;
static float batFrac = -1.0f;

static const uint16_t C_RX = TFT_WHITE, C_TX = TFT_CYAN, C_SYS = TFT_YELLOW,
                      C_RELAY = TFT_DARKGREY, C_ERR = TFT_RED;

void logLine(const String& s, uint16_t color) {
  logRing[logHead] = s;
  logColor[logHead] = color;
  logHead = (logHead + 1) % LOG_LINES;
  if (logCount < LOG_LINES) logCount++;
  if (scrollBack == 0) uiDirty = true;  // pinned to latest → repaint
}

// ─── Trackball → scroll (pulse counting in ISRs; consumed in loop) ───────────────
volatile int tbUp = 0, tbDown = 0;
ICACHE_RAM_ATTR void isrTbUp()   { tbUp++; }
ICACHE_RAM_ATTR void isrTbDown() { tbDown++; }

// ─── Setup ───────────────────────────────────────────────────────────────────────
void setup() {
  // T-Deck: nothing answers until the board-wide power gate is raised.
  pinMode(PIN_POWERON, OUTPUT);
  digitalWrite(PIN_POWERON, HIGH);
  delay(200);

  Serial.begin(115200);

  // Park the SD card's chip-select so it can't chatter on the shared bus.
  pinMode(PIN_SDCARD_CS, OUTPUT);
  digitalWrite(PIN_SDCARD_CS, HIGH);

  // Shared SPI up first, then the display claims it via LovyanGFX's lock.
  SPI.begin(PIN_SPI_SCK, PIN_SPI_MISO, PIN_SPI_MOSI, PIN_RADIO_CS);
  tft.init();
  tft.setBrightness(160);
  tft.fillScreen(TFT_BLACK);
  tft.setTextSize(1);

  // Keyboard (ESP32-C3 I2C slave). INT is optional — we poll at ~50 Hz anyway.
  Wire.begin(PIN_I2C_SDA, PIN_I2C_SCL);
  pinMode(PIN_KB_INT, INPUT_PULLUP);

  // Trackball: count pulses; up/down = scroll. (Left/right reserved.)
  pinMode(PIN_TB_UP, INPUT_PULLUP);
  pinMode(PIN_TB_DOWN, INPUT_PULLUP);
  attachInterrupt(digitalPinToInterrupt(PIN_TB_UP), isrTbUp, FALLING);
  attachInterrupt(digitalPinToInterrupt(PIN_TB_DOWN), isrTbDown, FALLING);

  // Node id: low byte of the STA MAC — same convention as the other OBC firmware.
  uint8_t mac[6];
  esp_read_mac(mac, ESP_MAC_WIFI_STA);
  nodeId = mac[5];

#if HAS_GPS
  // GPS UART. u-blox (M10Q SKU) default 38400; Quectel L76K SKU 9600 — try both.
  Serial1.begin(38400, SERIAL_8N1, PIN_GPS_RX, PIN_GPS_TX);
#endif

  if (!radioBegin()) {
    logLine("[radio] init FAILED — check antenna/pins", C_ERR);
  } else {
    radio.setPacketReceivedAction(onPacketReceived);
    radio.startReceive();
    char b[64];
    snprintf(b, sizeof(b), "[tdeck-%02X] %s net ready @ %.1f MHz", nodeId,
             netMode == NET_SPINE ? "spine" : "fleet", RADIO_FREQ_MHZ);
    logLine(b, C_SYS);
  }
  logLine("/help for commands. Plain text = chat.", C_SYS);
}

// (Re)configure the radio for the selected net. Params must match the host side:
// spine ↔ firmware/heltec-lora-linktest, fleet ↔ firmware/lora-node + lora_mesh.rs.
bool radioBegin() {
  int state;
  if (netMode == NET_SPINE) {
    state = radio.begin(RADIO_FREQ_MHZ, 125.0, 7, 5, 0x12, RADIO_POWER_DBM, 8);
  } else {
    state = radio.begin(RADIO_FREQ_MHZ, 125.0, 10, 5, 0x2B, RADIO_POWER_DBM, 8);
  }
  return state == RADIOLIB_ERR_NONE;
}

// ─── Main loop ───────────────────────────────────────────────────────────────────
void loop() {
  pumpRadio();
  pumpKeyboard();
  pumpTrackball();
  pumpUsb();
#if HAS_GPS
  pumpGps();
#endif
  maybeHeartbeat();
  maybeReadBattery();
  drawUi();
}

// ─── Radio RX: display + USB gateway print + flood relay ────────────────────────
void pumpRadio() {
  if (!packetReady) return;
  packetReady = false;

  uint8_t buf[MAX_PAYLOAD + SPINE_HEADER + 1];
  int len = radio.getPacketLength();
  if (len <= 0 || (size_t)len > sizeof(buf) - 1) { radio.startReceive(); return; }
  int state = radio.readData(buf, len);
  lastRssi = (int)radio.getRSSI();
  radio.startReceive();
  if (state != RADIOLIB_ERR_NONE) return;
  buf[len] = 0;
  statusDirty = true;

  if (netMode == NET_FLEET) {
    // Fleet codec: the payload IS the MeshFrame JSON. Show it + hand it to a
    // tethered host in obc_lora_bridge's node→host framing (payload + '\n').
    logLine(String("◄ ") + (const char*)buf, C_RX);
    Serial.write(buf, len);
    Serial.write('\n');
    return;
  }

  // Spine net: [src][seq][ttl][payload]
  if (len < (int)SPINE_HEADER) return;
  uint8_t src = buf[0], seq = buf[1], ttl = buf[2];
  const char* payload = (const char*)(buf + SPINE_HEADER);
  if (seenOrInsert(src, seq)) return;  // duplicate / already relayed

  char head[24];
  snprintf(head, sizeof(head), "◄%02X %ddBm ", src, lastRssi);
  logLine(String(head) + payload, C_RX);

  // Base-station console format — parsed verbatim by src/spine/lora_gateway.rs.
  Serial.printf("SPINE ◄ src=%02X seq=%u rssi=%d dBm : %s\n", src, seq, lastRssi, payload);

  // Flood relay onward, preserving ORIGINAL src/seq so de-dup works everywhere.
  if (ttl > 0) {
    uint8_t out[MAX_PAYLOAD + SPINE_HEADER];
    size_t n = len - SPINE_HEADER;
    out[0] = src; out[1] = seq; out[2] = ttl - 1;
    memcpy(out + SPINE_HEADER, buf + SPINE_HEADER, n);
    if (radio.transmit(out, n + SPINE_HEADER) == RADIOLIB_ERR_NONE) {
      char r[40];
      snprintf(r, sizeof(r), "⇒ relay %02X seq=%u ttl=%u", src, seq, ttl - 1);
      logLine(r, C_RELAY);
    }
    radio.startReceive();
  }
}

// Transmit `payload` as a frame we originate. Spine net wraps it in a header;
// fleet net sends the bytes raw (obc_lora_bridge / lora_mesh.rs compatible).
bool txPayload(const char* payload, size_t n) {
  if (n > MAX_PAYLOAD) { logLine("[tx] too long, dropped", C_ERR); return false; }
  int state;
  if (netMode == NET_SPINE) {
    uint8_t out[MAX_PAYLOAD + SPINE_HEADER];
    txSeq++;
    seenOrInsert(nodeId, txSeq);  // don't relay our own frame back
    out[0] = nodeId; out[1] = txSeq; out[2] = SPINE_TTL;
    memcpy(out + SPINE_HEADER, payload, n);
    state = radio.transmit(out, n + SPINE_HEADER);
  } else {
    state = radio.transmit((uint8_t*)payload, n);
  }
  radio.startReceive();
  if (state != RADIOLIB_ERR_NONE) {
    char e[32];
    snprintf(e, sizeof(e), "[tx] radio err %d", state);
    logLine(e, C_ERR);
    return false;
  }
  return true;
}

// ─── USB: a tethered OBC host's outbound lines → the mesh ───────────────────────
// Mirrors the gateway firmware: each newline-terminated line from the host (a
// NodeCommand from `mesh_command`, or a fleet MeshFrame from SerialMeshRadio) is
// framed onto LoRa unchanged.
static char usbBuf[MAX_PAYLOAD + 1];
static size_t usbLen = 0;
static bool usbOverflow = false;

void pumpUsb() {
  while (Serial.available() > 0) {
    char c = (char)Serial.read();
    if (c == '\n' || c == '\r') {
      if (usbLen > 0 && !usbOverflow) {
        usbBuf[usbLen] = 0;
        if (txPayload(usbBuf, usbLen)) logLine(String("►host ") + usbBuf, C_TX);
      }
      usbLen = 0; usbOverflow = false;
    } else if (usbLen < MAX_PAYLOAD) {
      usbBuf[usbLen++] = c;
    } else {
      usbOverflow = true;  // longer than one LoRa frame — drop, don't fragment
    }
  }
}

// ─── Keyboard: compose line + slash commands ─────────────────────────────────────
void pumpKeyboard() {
  static unsigned long lastPoll = 0;
  if (millis() - lastPoll < 20) return;  // ~50 Hz poll
  lastPoll = millis();

  Wire.requestFrom(KB_I2C_ADDR, (uint8_t)1);
  if (!Wire.available()) return;
  char c = Wire.read();
  if (c == 0) return;

  if (c == '\r' || c == '\n') {          // Enter → send
    if (composeBuf.length() > 0) {
      handleCompose(composeBuf);
      composeBuf = "";
    }
  } else if (c == 0x08) {                // Backspace
    if (composeBuf.length() > 0) composeBuf.remove(composeBuf.length() - 1);
  } else if ((uint8_t)c >= 0x20 && (uint8_t)c < 0x7F) {
    if (composeBuf.length() < 180) composeBuf += c;
  }
  uiDirty = true;
}

// Minimal JSON string escaping for chat text (quotes + backslash + control chars).
String jsonEscape(const String& s) {
  String out;
  out.reserve(s.length() + 8);
  for (size_t i = 0; i < s.length(); i++) {
    char c = s[i];
    if (c == '"' || c == '\\') { out += '\\'; out += c; }
    else if ((uint8_t)c < 0x20) { out += ' '; }
    else out += c;
  }
  return out;
}

void handleCompose(const String& lineIn) {
  String line = lineIn;
  line.trim();
  if (line.length() == 0) return;

  if (line.startsWith("/help")) {
    logLine("/cmd <to> <cmd> [json-args] — gated node command", C_SYS);
    logLine("/hb — toggle heartbeats   /net — spine|fleet", C_SYS);
    logLine("plain text — chat to the mesh", C_SYS);
    return;
  }
  if (line.startsWith("/hb")) {
    hbEnabled = !hbEnabled;
    logLine(hbEnabled ? "[hb] heartbeats ON" : "[hb] heartbeats OFF", C_SYS);
    return;
  }
  if (line.startsWith("/net")) {
    netMode = (netMode == NET_SPINE) ? NET_FLEET : NET_SPINE;
    if (radioBegin()) {
      radio.setPacketReceivedAction(onPacketReceived);
      radio.startReceive();
      logLine(netMode == NET_SPINE ? "[net] spine (SF7/0x12)" : "[net] fleet (SF10/0x2B)", C_SYS);
    } else {
      logLine("[net] radio re-init FAILED", C_ERR);
    }
    statusDirty = true;
    return;
  }
  if (line.startsWith("/cmd ")) {
    // /cmd <to> <cmd> [json-args]  → NodeCommand line (see lora_gateway.rs).
    // The target node's on-MCU Track 0 gate decides whether it actually runs.
    String rest = line.substring(5);
    rest.trim();
    int sp1 = rest.indexOf(' ');
    if (sp1 < 0) { logLine("[cmd] usage: /cmd <to> <cmd> [json-args]", C_ERR); return; }
    String to = rest.substring(0, sp1);
    String rest2 = rest.substring(sp1 + 1);
    rest2.trim();
    int sp2 = rest2.indexOf(' ');
    String cmd = sp2 < 0 ? rest2 : rest2.substring(0, sp2);
    String args = sp2 < 0 ? "{}" : rest2.substring(sp2 + 1);
    args.trim();
    if (!args.startsWith("{")) args = "{}";  // args must be a JSON object

    char out[MAX_PAYLOAD + 1];
    int n = snprintf(out, sizeof(out),
                     "{\"id\":\"td-%02X-%lu\",\"to\":\"%s\",\"cmd\":\"%s\",\"args\":%s}",
                     nodeId, (unsigned long)++cmdSeq, to.c_str(), cmd.c_str(), args.c_str());
    if (n < 0 || (size_t)n >= sizeof(out)) { logLine("[cmd] too long", C_ERR); return; }
    if (txPayload(out, n)) logLine(String("► ") + out, C_TX);
    return;
  }

  // Plain text → chat message the host gateway ingests into world memory.
  char out[MAX_PAYLOAD + 1];
  int n;
  if (netMode == NET_FLEET) {
    // No chat frame in the fleet codec — send as an idle heartbeat w/ the text
    // in the mode field so it at least surfaces host-side; spine net is richer.
    n = snprintf(out, sizeof(out), "{\"t\":\"hb\",\"n\":\"tdeck-%02X\",\"m\":\"chat:%s\"}",
                 nodeId, jsonEscape(line).c_str());
  } else {
    n = snprintf(out, sizeof(out), "{\"node_id\":\"tdeck-%02X\",\"type\":\"chat\",\"text\":\"%s\"}",
                 nodeId, jsonEscape(line).c_str());
  }
  if (n < 0 || (size_t)n >= sizeof(out)) { logLine("[chat] too long", C_ERR); return; }
  if (txPayload(out, n)) logLine(String("► ") + line, C_TX);
}

// ─── Trackball → scrollback ──────────────────────────────────────────────────────
void pumpTrackball() {
  int up = 0, down = 0;
  noInterrupts();
  up = tbUp; tbUp = 0;
  down = tbDown; tbDown = 0;
  interrupts();
  if (up == 0 && down == 0) return;
  scrollBack += up;            // roll up → older lines
  scrollBack -= down;          // roll down → newer
  if (scrollBack < 0) scrollBack = 0;
  int maxBack = logCount > 20 ? logCount - 20 : 0;
  if (scrollBack > maxBack) scrollBack = maxBack;
  uiDirty = true;
}

// ─── GPS / battery / heartbeat ───────────────────────────────────────────────────
#if HAS_GPS
void pumpGps() {
  static unsigned long baudSwitchAt = 15000;
  static bool triedAltBaud = false;
  while (Serial1.available() > 0) gps.encode(Serial1.read());
  // No NMEA at 38400 after 15 s? Retry at 9600 (Quectel L76K SKU).
  if (!triedAltBaud && millis() > baudSwitchAt && gps.charsProcessed() < 10) {
    triedAltBaud = true;
    Serial1.updateBaudRate(9600);
    logLine("[gps] no NMEA @38400, trying 9600 (L76K?)", C_SYS);
  }
}
#endif

void maybeReadBattery() {
  static unsigned long lastRead = 0;
  if (millis() - lastRead < 10000) return;
  lastRead = millis();
  // Divider halves the pack voltage; 12-bit ADC, 3.3 V ref, 11 dB attenuation.
  uint32_t mv = analogReadMilliVolts(PIN_BAT_ADC) * 2;
  // Rough LiPo curve: 3.30 V empty → 4.20 V full.
  float f = ((float)mv / 1000.0f - 3.3f) / 0.9f;
  batFrac = f < 0 ? 0 : (f > 1 ? 1 : f);
  statusDirty = true;
}

void maybeHeartbeat() {
  if (!hbEnabled) return;
  unsigned long nowMs = millis();
  if (nowMs - lastHbMs < HB_PERIOD_MS) return;
  lastHbMs = nowMs;

  char out[MAX_PAYLOAD + 1];
  int n;
  bool fix = false;
  double lat = 0, lon = 0;
#if HAS_GPS
  fix = gps.location.isValid();
  if (fix) { lat = gps.location.lat(); lon = gps.location.lng(); }
#endif
  if (netMode == NET_FLEET) {
    // Fleet codec heartbeat → drops straight into the fleet Coordinator.
    if (fix) {
      n = snprintf(out, sizeof(out),
                   "{\"t\":\"hb\",\"n\":\"tdeck-%02X\",\"x\":%.6f,\"y\":%.6f,\"b\":%.2f,\"m\":\"handheld\"}",
                   nodeId, lon, lat, batFrac < 0 ? 1.0f : batFrac);
    } else {
      n = snprintf(out, sizeof(out),
                   "{\"t\":\"hb\",\"n\":\"tdeck-%02X\",\"b\":%.2f,\"m\":\"handheld\"}",
                   nodeId, batFrac < 0 ? 1.0f : batFrac);
    }
  } else {
    // Spine heartbeat → host gateway ingests it into world memory.
    if (fix) {
      n = snprintf(out, sizeof(out),
                   "{\"node_id\":\"tdeck-%02X\",\"type\":\"hb\",\"lat\":%.6f,\"lon\":%.6f,\"battery\":%.2f,\"mode\":\"handheld\"}",
                   nodeId, lat, lon, batFrac < 0 ? 1.0f : batFrac);
    } else {
      n = snprintf(out, sizeof(out),
                   "{\"node_id\":\"tdeck-%02X\",\"type\":\"hb\",\"battery\":%.2f,\"mode\":\"handheld\"}",
                   nodeId, batFrac < 0 ? 1.0f : batFrac);
    }
  }
  if (n > 0 && (size_t)n < sizeof(out)) txPayload(out, n);
}

// ─── UI ──────────────────────────────────────────────────────────────────────────
// 320x240 landscape: 14px status bar / 198px log (18 lines @ 11px) / 28px compose.
void drawUi() {
  if (statusDirty) {
    tft.fillRect(0, 0, 320, 14, TFT_NAVY);
    tft.setTextColor(TFT_WHITE, TFT_NAVY);
    tft.setCursor(2, 3);
    char s[80];
    char gpsTag[12] = "gps:-";
#if HAS_GPS
    snprintf(gpsTag, sizeof(gpsTag), gps.location.isValid() ? "gps:FIX" : "gps:...");
#endif
    snprintf(s, sizeof(s), "tdeck-%02X %s %s bat:%d%% rssi:%d",
             nodeId, netMode == NET_SPINE ? "spine" : "fleet", gpsTag,
             batFrac < 0 ? -1 : (int)(batFrac * 100), lastRssi);
    tft.print(s);
    statusDirty = false;
  }
  if (!uiDirty) return;
  uiDirty = false;

  // Log area
  tft.fillRect(0, 14, 320, 198, TFT_BLACK);
  int lines = 18;
  int newest = logCount - 1 - scrollBack;
  for (int row = lines - 1; row >= 0 && newest >= 0; row--, newest--) {
    int idx = (logHead - logCount + newest + 2 * LOG_LINES) % LOG_LINES;
    tft.setTextColor(logColor[idx], TFT_BLACK);
    tft.setCursor(2, 16 + row * 11);
    // Truncate to one row; the full payload still went to USB for the host.
    String s = logRing[idx];
    if (s.length() > 52) s = s.substring(0, 51) + "…";
    tft.print(s);
  }
  if (scrollBack > 0) {
    tft.setTextColor(TFT_YELLOW, TFT_BLACK);
    tft.setCursor(280, 16);
    tft.printf("↑%d", scrollBack);
  }

  // Compose line
  tft.fillRect(0, 212, 320, 28, TFT_BLACK);
  tft.drawFastHLine(0, 212, 320, TFT_DARKGREY);
  tft.setTextColor(TFT_GREEN, TFT_BLACK);
  tft.setCursor(2, 220);
  String c = String("> ") + composeBuf + "_";
  if (c.length() > 52) c = "> …" + composeBuf.substring(composeBuf.length() - 46) + "_";
  tft.print(c);
}
