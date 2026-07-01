# OBC LoRa-mesh node firmware

Reference firmware that turns a LoRa dev board (LilyGO **T-Beam**, **Heltec WiFi
LoRa 32 v2**, or **RAK4631**) into a transparent **USB-serial ⇄ LoRa** bridge for
the Oh-Ben-Claw fleet mesh.

## The split

The node is a **dumb radio modem**. It relays opaque bytes and knows nothing about
their meaning. All mesh semantics live on the OBC host:

```
 fleet::Coordinator  ── NodeState ──►  lora_mesh::SerialMeshRadio  ── USB serial ──►  [ node ]  ── LoRa ──►  other nodes
 fleet::Coordinator  ◄─ report() ───  lora_mesh::run_serial_rx    ◄─ USB serial ──   [ node ]  ◄─ LoRa ───  other nodes
```

Keeping the radio dumb means the frame format (`MeshFrame` in
`src/spine/lora_mesh.rs`) can evolve host-side **without reflashing radios**.

## Wire protocol (must match the host)

Newline-delimited, one compact-JSON `MeshFrame` per line:

| direction     | on the wire                                                        |
|---------------|-------------------------------------------------------------------|
| host → node   | `{"t":"hb","n":"rover-a","x":1.0,"y":2.0,"b":0.9,"m":"explore"}\n` → transmitted over LoRa (without the `\n`) |
| node → host   | each received LoRa packet's bytes, then `\n`, written to USB serial |

`t` is `hb` (heartbeat) or `as` (assignment); `x`/`y`/`b` are optional. The host's
`run_serial_rx` decodes each inbound line and calls `Coordinator.report(...)`, so a
heartbeat heard over the air becomes a `fleet::NodeState` and drops straight into
the auction/exploration logic — unchanged.

## Flash it

1. Arduino IDE (or `arduino-cli`) with the ESP32 (T-Beam/Heltec) or nRF52
   (RAK4631) board package installed.
2. Library Manager → install **RadioLib** by Jan Gromes (6.x).
3. Open `obc_lora_bridge/obc_lora_bridge.ino`.
4. At the top of the sketch, **uncomment your board** (`BOARD_TBEAM_SX1276`,
   `BOARD_HELTEC_V2_SX1276`, or `BOARD_RAK4631_SX1262`) and comment the others.
5. Set `RADIO_FREQ_MHZ` for your **region** (US ISM `915.0`, EU868 `868.0`). Every
   node in the mesh must share frequency **and** `RADIO_BW_KHZ` / `RADIO_SF` /
   `RADIO_CR` / `RADIO_SYNCWORD`, or they won't hear each other.
6. Verify the pin map against your board revision (values in the sketch are the
   common community pinouts, not guaranteed for every hardware rev).
7. Upload.

## Bring-up (end to end)

On the OBC host, open the node's serial port with the matching baud (115200) and
spawn the RX loop against your fleet `Coordinator` — behind the `hardware` feature:

```rust
// pseudocode for the main/config wiring (still TODO on the host side)
let (radio, rx) = SerialMeshRadio::open("/dev/ttyUSB0", 115200)?;
tokio::spawn(run_serial_rx(rx, coordinator.clone(), || now_ms()));
// `radio` implements MeshRadio: coordinator assignments -> transmit() -> LoRa
```

Quick sanity check **without** the host: open the port in any serial monitor at
115200 and type a heartbeat line —

```
{"t":"hb","n":"bench","x":0,"y":0,"m":"idle"}
```

— pressing Enter. A second node in range, wired to its own serial monitor, should
print that exact line. That round-trip proves the radio link and the framing before
you involve the fleet coordinator.

## What this is / isn't

* **Is:** a minimal, portable byte-relay matching OBC's serial framing; a real
  RadioLib radio driver for three common boards.
* **Isn't:** a Meshtastic-protobuf client. It speaks OBC's own compact `MeshFrame`
  codec, not the Meshtastic packet format. It also does no mesh **routing** — it's a
  single-hop broadcast bridge. Multi-hop relaying, if needed, is a host-side concern
  (rebroadcast with a TTL) layered on top of this transport.
* **Untested by its author.** Mirrors RadioLib 6.x's documented API and OBC's
  framing; pin maps and radio params are starting points to verify on your bench.
