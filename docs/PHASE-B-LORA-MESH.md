# Phase B — LoRa Mesh Spine (Heltec V3)

Off-grid inter-node transport for Oh-Ben-Claw: OBC nodes exchange their
newline-delimited JSON spine messages (link state, power mode, reflex/safing
reports) over a **LoRa mesh** when there's no WiFi/MQTT backhaul. Validated on
hardware (2× Heltec WiFi LoRa 32 V3, 1× Seeed XIAO ESP32-S3).

## Architecture — serial-bridged compute

The compute node and the radio are separate boards, bridged by a UART:

```
  XIAO ESP32-S3 (node)              Heltec V3 (LoRa gateway)         Heltec V3 (base / peer)
  ┌──────────────────┐   UART1      ┌──────────────────────┐  LoRa  ┌────────────────────┐
  │ sensors, reflexes│  D6(GPIO43)  │ SX1262 915 MHz radio │ 915MHz │ SX1262 radio       │
  │ safing (System 1)│ ───TX──────► │ UART1 RX = GPIO2     │◄──────►│ spine frames +     │
  │ mirrors JSON out │   GND ─ GND  │ frames + flood-relay │        │ flood-relay + dedup│
  └──────────────────┘              └──────────────────────┘        └────────────────────┘
```

- The **XIAO** runs the full node firmware (`firmware/obc-esp32-s3`) and *mirrors*
  its autonomous spine messages out a UART.
- The **Heltec** runs the gateway firmware (`firmware/heltec-lora-linktest`): it
  frames UART lines onto LoRa, forwards received frames back to the UART, and
  flood-relays for the mesh. The XIAO has no LoRa; the Heltec has no sensors.

## Hardware

### Heltec WiFi LoRa 32 V3 (ESP32-S3 + SX1262)

| Function | GPIO | Notes |
|---|---|---|
| SX1262 NSS (CS) | 8 | SPI2 |
| SX1262 SCK | 9 | |
| SX1262 MOSI | 10 | |
| SX1262 MISO | 11 | |
| SX1262 RST | 12 | |
| SX1262 BUSY | 13 | command handshake |
| SX1262 DIO1 | 14 | IRQ |
| SX1262 TCXO | — | on the chip's DIO3, 1.8 V |
| SX1262 RF switch | — | on the chip's DIO2 (chip-controlled) |
| UART1 (compute uplink) TX | 4 | to node RX (future return path) |
| UART1 (compute uplink) RX | 2 | ← node TX |
| USB console (CP2102) | 43/44 | UART0 — **needs the Silicon Labs CP210x driver on Windows** |
| LED | 35 | |

### Seeed XIAO ESP32-S3 (node)

| Function | GPIO / pad | Notes |
|---|---|---|
| Spine mirror TX | GPIO43 / **D6** | → Heltec GPIO2 |
| USB console | native USB-Serial-JTAG (19/20) | shows as "USB Serial Device" |

## Radio configuration

- **Band:** 915 MHz (US ISM). Change `FREQ_HZ` for EU (868 MHz).
- **Modulation:** SF7 / BW 125 kHz / CR 4-5.
- **Sync word:** `0x1424` (private network — both nodes must match).
- **TX power:** +22 dBm. **Attach an antenna before transmitting** — +22 dBm into
  no antenna can damage the SX1262 PA.

## Spine frame format

Content-agnostic transport (`spine.rs`). Single frame:

```
[src:u8][seq:u8][ttl:u8][payload…]
```

- `src` — originating node id (low byte of its MAC).
- `seq` — per-source sequence (wraps); with `src`, de-dups relays.
- `ttl` — remaining hop count for flood-relay (`SPINE_TTL` = 2; 0 = don't relay).
- `payload` — the OBC message bytes (≤ 240 B).

**De-dup / flood-relay:** a `SeenSet` ring records recently-seen `(src, seq)`. A node
that hears a *new* frame forwards it to its UART, then rebroadcasts it with `ttl-1`
(preserving the original `src`/`seq`). Any node that has already seen `(src, seq)`
drops it — that's what stops relay loops.

## Build & flash

Toolchain: the Espressif Xtensa Rust toolchain (`espup install`), same as the node
firmware. Both firmware crates are their own workspaces (excluded from the host
workspace).

```powershell
# Heltec gateway firmware
cd firmware/heltec-lora-linktest
$env:ESPFLASH_PORT="COM4"; cargo run --release   # set the port explicitly per board

# XIAO node firmware (includes the spine mirror)
cd firmware/obc-esp32-s3
$env:ESPFLASH_PORT="COM3"; cargo run --release
```

> **`ESPFLASH_PORT` persists for the whole terminal session** — always set it to the
> target board's port before each `cargo run`, or a flash can land on the wrong board.
> Identify ports with `Get-CimInstance Win32_SerialPort | Select DeviceID,Description`:
> CP210x = Heltec, "USB Serial Device" = XIAO.

## Wiring (compute node → gateway)

Two jumpers, one-directional (node transmits its JSON to the gateway):

| XIAO | → | Heltec gateway |
|---|---|---|
| **D6** (GPIO43) | → | **GPIO2** |
| **GND** | → | **GND** |

A shared ground is mandatory — a missing common GND is the #1 UART failure and
looks like "no data". Verify both connections with a multimeter on continuity.

## Test procedure

1. **Both Heltecs** — flash the gateway firmware, confirm each boots with
   `SX1262 self-test: ... syncword readback=0x1424`. They exchange `gw_keepalive`
   spine frames (`SPINE ◄ src=.. : {"type":"gw_keepalive",...}`), and each shows a
   `SPINE ⇒ relay` line per received frame (flood-relay, capped one-per-node).
2. **XIAO** — flash the node firmware; its boot logs `Spine uplink: UART1 ready`.
3. **Wire** the XIAO to one Heltec (that one is the *gateway*; the other is the
   *base station*).
4. **Watch the base station** (`espflash monitor --port COMx`). Once the XIAO's link
   goes offline (~30 s untethered), its `safe-link-offline` reflex fires every 10 s
   and appears at the base station over LoRa:
   ```
   SPINE ◄ src=<gw> seq=N rssi=-X dBm : {"type":"reflex","node_id":"obc-esp32-s3-001",...}
   ```
   `node_id":"obc-esp32-s3-001"` = the XIAO's real on-MCU reflex, relayed over the mesh.

## Host ⇄ mesh (inbound bridge)

The far end of the spine: a **base-station Heltec** plugged into the host machine's USB
becomes the host brain's window onto the mesh. `src/spine/lora_gateway.rs` reads that
Heltec's console and ingests every node spine message it hears over the air into
**world memory** — so a reflex or link-state report that travelled node → gateway →
LoRa → base station lands in the brain's world model, exactly as if the node were on the
wired MQTT spine.

```
  base-station Heltec (USB)          host (oh-ben-claw)
  ┌────────────────────┐  console   ┌───────────────────────────────┐
  │ SPINE ◄ src=.. : {} │ ─────────► │ lora_gateway::run_gateway_rx  │
  │ (every RX frame)    │  115200    │  parse → observe() → world.db │
  └────────────────────┘            └───────────────────────────────┘
```

Each received line writes two facts (valid *now*, source `lora-gateway`):

| Entity | Value |
|---|---|
| `mesh.<node_id>.<type>` | the node payload + a `_mesh` envelope (`src`, `seq`, `rssi_dbm`) |
| `mesh.<node_id>` | liveness/link rollup — `rssi_dbm`, `seq`, `src`, `last_type` |

So `current("mesh.obc-esp32-s3-001")` answers *"is this node alive, and how strong is
the mesh link?"*, and `history("mesh.obc-esp32-s3-001.reflex")` gives the reflex trail.

### Config

```toml
[perception]
world_memory = true          # the bridge needs somewhere to write

[lora_gateway]
port = "COM6"                # the base-station Heltec's USB console
baud = 115200
```

The serial loop is gated behind the `hardware` feature (tokio-serial), like the other
peripheral drivers. Run the host with it enabled:

```powershell
cargo run --features hardware
```

Without `--features hardware` the config is accepted but the bridge logs a warning and
doesn't start (parse/ingest still compile and are unit-tested). The parse + ingest core
is hardware-free — `cargo test spine::lora_gateway` covers it on any machine.

## Host ⇄ mesh (outbound return path)

The inverse direction: a command originated on the host reaches a node over LoRa. This
turns the mesh into a genuine two-way link.

```
  host (mesh_command tool)          base Heltec (USB)      gateway Heltec        XIAO node
  ┌──────────────────────┐  serial  ┌────────────────┐ LoRa ┌──────────────┐ UART ┌────────────┐
  │ NodeCommand{to,cmd,…} │ ───────► │ console → LoRa │ ───► │ LoRa → UART1 │ ───► │ D7 (GPIO44)│
  │ SerialCommandSink     │  115200  │  (firmware*)   │      │  TX = GPIO4  │      │ handle_req │
  └──────────────────────┘          └────────────────┘      └──────────────┘      │  (gated)   │
                                                              GPIO4 → XIAO D7      └────────────┘
```

- The agent calls **`mesh_command`** `{ node_id, command, args }`. It encodes a
  `NodeCommand` to the node's own request line (`{"id","to","cmd","args"}`) and writes it
  to the base-station Heltec over the *same* serial port the inbound bridge reads
  (`open_split` shares it).
- The **node** drains its spine UART RX (GPIO44 / **D7**), checks the `to` field matches
  its `NODE_ID` (or is absent = broadcast), and dispatches through the **same Track 0–gated
  `handle_request`** as a USB command. So a `gpio_write` over the air actuates only within
  the node's on-MCU allow-list / range / rate limits — the node is the authoritative gate.
  Its reply is written back out the UART to ride LoRa home.

### Remaining to run outbound end-to-end

| Piece | State |
|---|---|
| Host `mesh_command` + `NodeCommand` + sink | ✅ unit-tested |
| Node UART1-RX intake → gated dispatch | ✅ firmware written (*flash-pending*) |
| Base-station Heltec: USB console → LoRa TX | ✅ firmware written (*flash-pending*) |
| **Reverse** jumper: gateway **GPIO4** → XIAO **D7** (GPIO44), GND↔GND | ⏳ new wire |

The base-station origin is a background thread that reads the Heltec's USB console
(UART0 `stdin`) and frames each line onto LoRa. It only *reads* stdin — it never installs
a UART0 driver, so the console/log output is untouched. To send a command, type or pipe a
JSON line into the base-station Heltec's serial monitor:

```json
{"to":"obc-esp32-s3-001","id":"h1","cmd":"gpio_write","args":{"pin":3,"value":1}}
```

> Fallback if a particular console setup won't deliver stdin: drive the base station's
> **UART1 RX (GPIO2)** from a USB-TTL adapter instead — the existing UART1→LoRa path
> needs no firmware change. Either way, the `mesh_command` agent tool writes the same
> line to whichever serial port the host has open.

## Status

| Piece | State |
|---|---|
| SX1262 driver (hand-rolled) | ✅ validated first flash |
| 2-node point-to-point LoRa link | ✅ RSSI/SNR confirmed |
| Structured spine frames + de-dup | ✅ |
| Heltec UART↔LoRa gateway bridge | ✅ |
| XIAO spine mirror (firmware) | ✅ UART1 init confirmed |
| Flood-relay (TTL + dedup) | ✅ no-loop confirmed |
| XIAO→Heltec physical jumper | ⏳ deferred (continuity check pending) |
| True 3-hop relay | ⏳ needs a 3rd radio out of direct range |

## Troubleshooting

- **`No serial ports detected` on a Heltec** → install the Silicon Labs CP210x VCP
  driver; the Heltec's USB is a CP2102 bridge (unlike the XIAO's native USB).
- **Flash lands on the wrong board** → `$env:ESPFLASH_PORT` is still set from a prior
  command. Set it explicitly each time.
- **COM numbers changed** → Windows renumbers on re-plug; re-run the `Get-CimInstance`
  query and adjust.
- **Base station sees only `gw_keepalive`, never the node's JSON** → the node→gateway
  UART isn't delivering. Check the node's boot log for `Spine uplink: UART1 ready`
  (rules out firmware), then the jumper: XIAO **D6** (not D7), landing on Heltec
  **GPIO2**, and a **shared GND**.
- **No RX between two radios** → confirm antennas are attached and, if the boards are
  touching, separate them ~1 m (a +22 dBm signal can desense a very close receiver).
- **Relay storm (same seq relayed repeatedly)** → would indicate the `SeenSet` isn't
  catching dups; expected behaviour is exactly one `⇒ relay` per `(src, seq)`.
