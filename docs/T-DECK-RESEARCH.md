# LilyGo T-Deck — Deep Research Report

*Compiled July 5, 2026. Claims verified against primary sources (lilygo.cc, Meshtastic docs, Xinyuan-LilyGO GitHub, and the T-Deck repo's own source). Contested claims went through an adversarial verification pass — verdicts noted inline. This copy lives in Oh-Ben-Claw because it is the factual basis for the T-Deck integration: the registry entries, `firmware/t-deck-terminal`, and the `obc_lora_bridge` T-Deck preset.*

---

## 1. Hardware: the three variants

| | T-Deck | T-Deck Plus | T-Deck Pro |
|---|---|---|---|
| MCU | ESP32-S3FN16R8 (dual LX7 @ 240 MHz, 16MB flash, 8MB PSRAM) | same | same |
| Display | 2.8" IPS 320×240, ST7789 (SPI), GT911 capacitive touch (I2C) | same | 3.1" e-paper 320×240 (GDEQ031T10), CST328 touch; ~3s full / ~0.5s partial refresh |
| Keyboard | QWERTY driven by a **separate ESP32-C3** as I2C slave @ 0x55 (INT on GPIO46), reflashable via unpopulated 6-pin header + USB-TTL | same | TCA8418 keypad-scan IC (no C3) |
| Trackball | Yes — 4× AN48841B Hall sensors (GPIO 1/2/3/15); center press = BOOT button | same | None |
| LoRa | Semtech SX1262, +22 dBm, 433/868/915 MHz SKUs; also sold "Without LoRa" | SX1262; adds 920 MHz MIC (Japan) SKU; internal or external antenna SKUs | SX1262; 433/868/915/920; integrated PCB antenna |
| GPS | None onboard — add via Grove UART (GPIO 43/44), official GPSShield example | **Onboard, SKU-dependent**: u-blox MIA-M10Q (38400 baud) *or* Quectel L76K (9600 baud) — GPS consumes the Grove pins, so Grove port unusable | u-blox MIA-M10Q |
| Battery | **Not included** — JST cable supplied, bring your own LiPo (ADC on IO04) | Built-in **2000 mAh** ✓verified — "5000 mAh" claims trace to Geerling's DIY original-T-Deck build, not the Plus | Built-in 1400 mAh, dedicated PMU, vibration motor |
| Audio | MAX98357A I2S amp + speaker; dual mics via ES7210 ADC | same | Either SIMCom A7682E LTE Cat-1 modem (handles audio) **or** PCM5102A DAC — mutually exclusive SKUs; adds 3.5mm jack |
| Cellular | No | No | 4G LTE Cat-1 (A7682E SKU only) |
| Expansion | Grove HY2.0-4P UART connector | present but consumed by GPS | Qwiic (I2C) |
| Storage / USB | microSD on shared SPI (CS GPIO39); USB-C (native S3 USB, charging via TP4065B) | same | same |
| Extra sensors | — | — | BHI260AP IMU, LTR-553ALS light sensor |
| Size | — | ~115×72×20 mm | ~120×66×13.5 mm |
| Price (lilygo.cc, Jul 2026) | from ~$43 | ~$61–71 | ~$85–95 |

All variants ship preflashed with a choice of LilyGo factory firmware, Meshtastic, or (Plus/Pro) MeshCore.

**Firmware gotchas** (from this repo / utilities.h): GPIO10 (`BOARD_POWERON`) must be driven HIGH before any peripheral works; display/LoRa/SD share one SPI bus (MOSI 41, MISO 38, SCK 40); keyboard I2C on SDA 18 / SCL 8; radio CS 9, BUSY 13, RST 17, DIO1 45. Never power on without a LoRa antenna attached.

## 2. Firmware ecosystem

**Meshtastic** (primary ecosystem) — officially supported; flash via flasher.meshtastic.org ("T Deck"). Current stable series is **2.7.x** (2.7.15 promoted stable; alphas/betas into 2.7.2x as of mid-2026). Since 2.7 you can switch at runtime between **BaseUI** and **MUI** — the LVGL/LovyanGFX touch UI (`meshtastic/device-ui`) with offline SD-card maps, keyboard/trackball input. Constraint: MUI occupies the single Client API connection, so no concurrent BLE phone-app link — switch to BaseUI or Bluetooth Programming Mode.

**MeshCore** — incompatible alternative mesh firmware; officially supports T-Deck Plus (flasher.meshcore.io) as a "Companion" node. Note: the full-featured T-Deck MeshCore firmware/MeshOS is **paid/proprietary**; community alternatives exist (Aurora, MCLite). Users multiboot Meshtastic + MeshCore via bmorcelli/Launcher with images on SD.

**LilyGo factory firmware** — this repo: UnitTest, lvgl_example, Keyboard master/slave, LoRaWAN_Starter, GPSShield, Microphone examples. Flash with esptool @ 921600, DIO, 80 MHz. Arduino IDE must pin ESP32 core to **2.0.14** (newer breaks TFT_eSPI); PlatformIO needs `-DBOARD_HAS_PSRAM=1`, `-DARDUINO_USB_CDC_ON_BOOT=1`, `psram_type = opi`.

**Others**: CircuitPython (official board support, 10.2.1 stable, UF2 or bin); Tulip Creative Computer (MicroPython/AMY/LVGL music-synth environment); Bruce (security-research firmware, official T-Deck support); acid-drop (pocket IRC/ChatGPT terminal, beta); Hack3r T-Deck (daily-driver UI); rsDeck & Pyxis (Reticulum/LXMF messengers + RNode mode); IceNav (offline GPS navigator). Mainline MicroPython board support: unconfirmed. Community index: github.com/battlehax/awesome-t-deck.

## 3. Uses

- **Off-grid messenger** — the flagship use: first truly standalone Meshtastic node (keyboard+screen, no phone). Field results: ~100 km Malta→Sicily with external antenna; 110 km user-reported node link. Typical real-world range is far lower: hundreds of m urban, ~5–15 km line-of-sight.
- **Emergency/preparedness comms** — Meshtastic meshes carried real traffic post-Hurricane Helene (2024); marketed to ham/preparedness communities as a go-bag communicator.
- **Ham radio** — LoRa-APRS tracker use; LoRaWAN possible via the repo's LoRaWAN_Starter (Meshtastic itself is P2P mesh, not LoRaWAN).
- **Cyberdeck / pocket computer** — CircuitPython/Tulip/acid-drop builds, IRC and cloud-LLM chat clients (no credible on-device LLM — it's an ESP32).
- **GPS navigation** — IceNav; Meshtastic MUI offline map tiles from SD.
- **Mods** — big 3D-printed case ecosystem (Printables/Thingiverse/MakerWorld): 18650/5000+ mAh battery mods, U.FL→SMA external antenna mods.

**Known limitations** (user-reported): internal-antenna Plus is "deaf" — buy external-antenna SKU; 2000 mAh ≈ 1–2 charges/day in active use; trackball widely panned; no RTC battery (time lost on power-off); slow ~0.5 A charging; awkward symbol typing and painful keyboard-C3 reflashing; no IP rating; bottom USB-C blocks upright charging; some units needed LilyGo's TouchFix binary; one bundled 433 MHz antenna measured resonant at ~496 MHz.

## 4. Connectivity

- **LoRa**: SX1262 (+22 dBm) over shared SPI. Factory default 868.0 MHz, BW 125 kHz, SF10, CR 4/6. U.FL/IPEX antenna connector (Plus internal antenna sits on the U.FL pigtail — fragile, don't overtighten SMA mods).
- **WiFi**: 2.4 GHz 802.11 b/g/n only (no 5 GHz). Under Meshtastic, enabling WiFi disables the BLE phone link.
- **Bluetooth**: BLE 5 only (ESP32-S3 has no BT Classic). PIN pairing with Meshtastic apps.
- **GPS**: UART GPIO 43/44 (Grove pins). Original: add-on shield; Plus/Pro: onboard (see §1).
- **USB-C**: native S3 USB — programming (hold trackball at power-on for download mode, or 1200bps auto-reset), serial, 5V charging.
- **Keyboard bus**: I2C to the ESP32-C3 (itself a WiFi/BLE-capable RISC-V MCU, stock firmware uses it only as an I2C peripheral).
- **Expansion**: Grove UART (original only, effectively), Qwiic on Pro; essentially **no spare GPIO** — everything is consumed by display/trackball/audio/SD/radio/keyboard.
- **4G**: Pro A7682E SKU only (LTE Cat-1, GSM/GPRS/EDGE fallback).

## 5. Integrations

**Runs ON the T-Deck**: Meshtastic firmware (MUI standalone UI, or BaseUI + Client API server over BLE/serial/TCP/HTTP), MQTT gateway over its own WiFi, or MeshCore companion firmware.

**Talks TO it**:
- **Phone/desktop apps** — official Meshtastic Android/iOS/macOS apps (BLE); web client (client.meshtastic.org) via HTTP(S)/Web Bluetooth/Web Serial (HTTP works on ESP32 devices like the T-Deck).
- **MQTT** — node uplinks/downlinks MeshPackets as ServiceEnvelope protobufs; public broker mqtt.meshtastic.org (meshdev/large4cats, zero-hop policy); "client proxy" mode relays MQTT through the phone's internet so the node needs no WiFi; JSON MQTT output is ESP32-only.
- **Home Assistant** — official meshtastic/home-assistant HACS integration (alpha 0.6.x): TCP/serial/BLE gateways, sensors, device_tracker, notify.mesh_*, bundled web client. Alternative pure-MQTT path officially documented. No maintained T-Deck ESPHome/HA-remote firmware exists.
- **ATAK/TAK** — official Meshtastic ATAK plugin (runs on the Android phone, not the deck): PLI + GeoChat over mesh; newer standalone plugin adds LT-coded file transfer and TAK-server relay; version pairing matters; iOS path is TAK-server-via-MQTT.
- **Mapping** — meshmap.net shows ~10k nodes via public-MQTT Map Reporting (default channel + OK-to-MQTT + coarse position required).
- **APIs** — `meshtastic` PyPI package (CLI + Serial/TCP/BLE interfaces, pub/sub); everything speaks the shared protobuf schema (meshtastic/protobufs).
- **Bridges** (host-side) — Telegram (meshgram, emtt), Discord, Node-RED (node-red-contrib-meshtastic), and MESH-API (Meshtastic+MeshCore router bridging to Ollama/LM Studio LLMs, HA, Twilio SMS, Discord, incl. an MCP server).
- **Hardware interop** — meshes with any Meshtastic node (T-Beam, T-Echo, Heltec V3, RAK4631, SenseCAP…) given matching region, modem preset, and channel key. MeshCore and Meshtastic nodes cannot talk to each other.

**Security notes**: channels use AES256-CTR with PSK — the default primary key ("AQ==") is public, so change it; DMs since 2.5.0 use X25519 + AES-CCM PKC; one disclosed advisory (GHSA-377p-prwp-4hwf) involved forged non-PKC DMs displaying as encrypted.

## 6. Verified verdicts on contested claims

1. **Plus battery = 2000 mAh** (not 5000). Confirmed via lilygo.cc + Meshtastic docs; 5000 mAh figures are DIY builds/mods.
2. **Meshtastic 2.7 is the current stable series** (2.7.15 promoted stable; 2.7.17 was revoked).
3. **Plus GPS is SKU-dependent** — u-blox MIA-M10Q or Quectel L76K; blanket "M10Q" claims are wrong for L76K units.
4. **Original T-Deck ships with no battery** (and optionally no LoRa). Confirmed from box contents.

## Sources

Primary: [lilygo.cc T-Deck](https://lilygo.cc/en-us/products/t-deck) · [T-Deck Plus](https://lilygo.cc/products/t-deck-plus-1) · [T-Deck Pro](https://lilygo.cc/en-us/products/t-deck-pro) · [Xinyuan-LilyGO/T-Deck](https://github.com/Xinyuan-LilyGO/T-Deck) (this repo) · [Meshtastic T-Deck docs](https://meshtastic.org/docs/hardware/devices/lilygo/tdeck/) · [Meshtastic firmware releases](https://github.com/meshtastic/firmware/releases) · [meshtastic/device-ui](https://github.com/meshtastic/device-ui) · [MQTT integration docs](https://meshtastic.org/docs/software/integrations/mqtt/) · [Encryption docs](https://meshtastic.org/docs/overview/encryption/) · [meshtastic/home-assistant](https://github.com/meshtastic/home-assistant) · [ATAK plugin](https://github.com/meshtastic/ATAK-Plugin) · [CircuitPython board page](https://circuitpython.org/board/lilygo_tdeck/)

Secondary: [CNX T-Deck Plus](https://www.cnx-software.com/2024/08/30/lilygo-t-deck-plus-a-blackberry-like-esp32-s3-devkit-with-qwerty-keyboard-trackball-lora-gps-battery-and-more/) · [CNX T-Deck Pro](https://www.cnx-software.com/2025/04/03/lilygo-t-deck-pro-esp32-s3-lora-messenger-e-paper-touch-display-keyboard-and-4g-lte-or-audio-codec-option/) · [Jeff Geerling](https://www.jeffgeerling.com/blog/2024/realizing-meshtastics-promise-t-deck/) · [OSSMalta review](https://ossmalta.eu/lilygo-t-deck-plus-review-a-meshtastic-handheld-with-great-potential-and-quirky-flaws/) · [andibond review aggregation](https://www.andibond.com/lilygo-t-deck-plus-esp32-s3-lora/) · [Hackaday MeshCore field test](https://hackaday.com/2025/12/06/lessons-learned-after-trying-meshcore-for-off-grid-text-messaging/) · [Liliputing](https://liliputing.com/lilygo-t-deck-plus-is-a-70-handheld-with-gps-lora-and-a-blackberry-keyboard/) · [awesome-t-deck](https://github.com/battlehax/awesome-t-deck)
