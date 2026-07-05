# t-deck-terminal — OBC handheld fleet console

Firmware that turns a **LilyGO T-Deck / T-Deck Plus** (ESP32-S3 + SX1262 +
QWERTY keyboard + 2.8" touch LCD) into an interactive, human-carried member of
the Oh-Ben-Claw LoRa spine. One device, three simultaneous roles:

| Role | What it does |
|---|---|
| **Console** | Screen shows a live scrollback of every spine frame heard over the air (heartbeats, reflex/safing reports, chat). Keyboard composes chat and node commands; trackball scrolls; status bar shows battery / GPS / RSSI / net. |
| **Gateway** | Tethered over USB, it prints every received frame in the exact `SPINE ◄ src=.. seq=.. rssi=.. dBm : {json}` console format that `src/spine/lora_gateway.rs` already parses, and frames the host's outbound `mesh_command` lines onto the mesh. **Drop-in replacement for the Heltec base station — zero host changes.** |
| **Relay** | Flood-relays spine frames with TTL decrement and `(src, seq)` de-dup, exactly like `firmware/heltec-lora-linktest`. |

Safety: the console has **no authority of its own**. A `/cmd` it sends is the
same `NodeCommand` line the host gateway path produces, and it executes only if
the *target node's* on-MCU Track 0 mirror (allow-list / range / rate) clears it.

## Two nets, one radio

| Net | On-air format | Radio params | Interoperates with |
|---|---|---|---|
| `spine` (default) | `[src][seq][ttl]` + JSON payload | SF7 / BW125 / CR4-5 / sync `0x12` | `firmware/heltec-lora-linktest`, `obc-esp32-s3` nodes, host `lora_gateway` |
| `fleet` | raw compact MeshFrame JSON | SF10 / BW125 / CR4-5 / sync `0x2B` | `firmware/lora-node` bridges, host `lora_mesh` fleet codec |

Toggle at runtime with `/net`. (RadioLib sync word `0x12` programs SX126x
registers `0x14 0x24` — the same value the Heltec gateway sets directly.)

## Keyboard commands

```
plain text                     chat → {"node_id":"tdeck-XX","type":"chat","text":...}
/cmd <to> <cmd> [json-args]    NodeCommand → {"id":"td-..","to":..,"cmd":..,"args":{..}}
/hb                            toggle 30 s heartbeats (GPS position on a Plus + battery)
/net                           switch spine ⇄ fleet net
/help                          command list
```

Heartbeats make the console itself visible to the brain: on the spine net they
land in world memory via the gateway; on the fleet net they drop straight into
the `fleet::Coordinator` like any rover's.

## Build & flash

1. Arduino IDE / `arduino-cli` with the **ESP32 board package** (espressif).
   Board: **ESP32S3 Dev Module** · PSRAM: **OPI PSRAM** · USB CDC On Boot:
   **Enabled** · Flash Size: **16MB**.
2. Library Manager: **RadioLib** (Jan Gromes, 6.x), **LovyanGFX** (lovyan03,
   1.1.x), **TinyGPSPlus** (Mikal Hart; only if `HAS_GPS 1`).
3. Open `t_deck_terminal/t_deck_terminal.ino`. Set:
   - `HAS_GPS` — `1` for a T-Deck Plus (or base T-Deck + Grove GPS shield),
     `0` for a base T-Deck. GPS baud is autodetected (u-blox M10Q SKU = 38400,
     Quectel L76K SKU = 9600).
   - `RADIO_FREQ_MHZ` — your region (US ISM `915.0`, EU868 `868.0`) and your
     hardware SKU's band. Must match every other node on the net.
4. **Attach the LoRa antenna** (transmitting without one can damage the SX1262).
5. Enter download mode: hold the **trackball center-press** (BOOT) while
   powering on, then upload.
6. Keyboard dead? The keyboard is its own ESP32-C3 running LilyGO's stock I2C
   firmware @ 0x55 — it is untouched by this flash, but if a previous project
   reflashed it, restore it per the Xinyuan-LilyGO/T-Deck README.

## Using it as the base station

Plug the T-Deck into the OBC host over USB-C and point the gateway at its port:

```rust
// same wiring as the Heltec base station — the console format is identical
spawn_gateway_serial("/dev/ttyACM0", 115200, world.clone());
```

`mesh_command` tool traffic flows out through it, and everything it hears —
including its own operator's chat — lands in world memory.

## Hardware notes (verified pin map)

Pins follow `Xinyuan-LilyGO/T-Deck` `utilities.h` (see also the repo's
`T-DECK-RESEARCH.md`): power gate **GPIO10 must go HIGH first**; shared SPI
(SCK 40 / MISO 38 / MOSI 41) carries the ST7789 display (CS 12, DC 11, BL 42),
SX1262 (CS 9, DIO1 45, RST 17, BUSY 13) and microSD (CS 39, parked HIGH);
keyboard = ESP32-C3 I2C slave @ `0x55` (SDA 18 / SCL 8, INT 46); trackball
pulses on 3/15/1/2; battery ADC on 4; GPS UART on 43/44 (Plus — this is why the
Plus's Grove port is unavailable).

**Status:** reference firmware — written against the verified pin map and the
documented RadioLib / LovyanGFX APIs, not yet bench-flashed. Validate the radio
parameters for your region before transmitting.
