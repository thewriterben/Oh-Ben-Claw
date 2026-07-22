# Bench Acceptance Run

**Purpose:** prove the mesh stack end to end, fast, with failures that localise to one
layer. Run it before trusting the bench, after any physical change, and as a regression
check when hardware behaviour is suspected during software work.

This is **not** the build guide — see `BENCH-WALKTHROUGH.md` for bring-up, and
`BENCH-PINOUT-CARDS.md` **Card 0** for board identity and wiring. This document assumes a
bench that has already worked once, and asks whether it still does.

**Why staged:** on 2026-07-19 a single loose jumper produced four hours of ambiguity,
because every test exercised the whole chain at once and failure looked identical at every
layer — silence. Each stage below isolates one layer and depends only on the stages above
it, so the first failing stage names the fault.

---

## Preconditions

| | |
|---|---|
| Board roles | fixed and labelled per Card 0 — base **gw-D8**, bridge **gw-40**, relay **gw-90** |
| Relay | **powered off** for stages A–G. A live relay makes the topology a three-radio flood and invalidates every RF reading |
| Power | powered USB hub. Two idle ESP32s will not keep a battery bank awake |
| Separation | base and bridge **3 m+ apart** for stages D–G; stage H deliberately violates this |
| Firmware | all three Heltecs on the current `heltec-lora-linktest` build; node on current `obc-esp32-s3` |

Ports re-enumerate on every replug. Node ids are MAC-derived and permanent. **When a label
and a log disagree, believe the log.**

---

## Stage summary

| Stage | Proves | A failure here means |
|---|---|---|
| **A** Identity | the boards are who you think | you are testing a different rig than you believe |
| **B** Loopback | bridge UART RX+TX, line framer | the bridge itself, not the node and not RF |
| **C** Uplink | node → bridge over the wire | the D6 jumper or the node's UART |
| **D** RF | base ↔ bridge radio link | antenna, distance, or saturation |
| **E** Ingest | frames → world memory → supervisor | host parser or the gateway bridge |
| **F** Command + Track 0 | full round trip, refusal scored healthy | the reverse leg, or the safety gate |
| **G** Framing | back-to-back commands stay intact | console reader or host pacing |
| **H** CRC | the saturation diagnostic fires | the CRC logging path |
| **I** Relay | true multi-hop | flood-relay or TTL handling |

Stages A–G are the trust gate. H and I are capability checks.

---

## Stage A — Identity

One board at a time on the hub. Attach a monitor, press **RST**, read the banner.

```powershell
[System.IO.Ports.SerialPort]::GetPortNames()
powershell -File scripts\serial-json-repl.ps1 -Port COM<n>
```

**Pass:**

```
Heltec V3 OBC spine gateway — LoRa 915 MHz ⇄ UART1 (compute uplink)
SX1262 self-test: status=0xA2, syncword readback=0x1424 (expect 0x1424)
Gateway 40 — UART1(TX=4,RX=2) ⇄ LoRa. Wire compute TX→GPIO2, GND↔GND.
```

Two hex digits matching Card 0, and a clean SX1262 self-test. A wrong syncword readback is
a radio fault and stops the run here.

---

## Stage B — Bridge loopback

Jumper **gw-40 GPIO4 → gw-40 GPIO2**. Nothing else. Power the relay **on** briefly as a
traffic source (the only stage where that is correct).

**Pass** — within ~5 s, repeatedly:

```
SPINE ◄ src=90 seq=59 rssi=-45 dBm snr=12 dB : {"node_id":"gw-90",...}
SPINE ⇒ relay src=90 seq=59 ttl=1
SPINE ► (uart) seq=1 (53 B) {"node_id":"gw-90","type":"gw_keepalive","seq":59}
```

Received → out GPIO4 → back in GPIO2 → framed → retransmitted. Byte count matches the
payload. This clears the bridge's UART in both directions and the `LineFramer` without
involving the node or the radio path under test.

Pull the loopback jumper and power the relay **off** before continuing.

---

## Stage C — Node uplink

Wire per Card 0: **XIAO D6 → gw-40 GPIO2**, **gw-40 GPIO4 → XIAO D7**, **GND ↔ GND**.
Monitor the bridge. Press **RST on the node**.

**Pass** — boot chatter dropped locally, real message transmitted:

```
uart: dropped non-OBC line (24 B) ESP-ROM:esp32s3-20210327
uart: dropped non-OBC line (48 B) rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)
...
SPINE ► (uart) seq=N (114 B) {"node_id":"obc-esp32-s3-001",...,"type":"link_state"}
```

Then a `beacon` every 30 s. Note the ROM log and the first JSON message often arrive
spliced with no newline; that line is correctly dropped and the beacon 30 s later is the
real confirmation.

> **Do not** conclude a jumper is good from a continuity test. On 2026-07-19 a wire passed
> continuity and could not carry 115200 baud. If this stage fails with B passing, replace
> the wire rather than measuring it.

---

## Stage D — RF baseline

Base and bridge both powered, 3 m+ apart, relay off. Watch the base.

**Pass:** keepalives every ~5 s, `rssi` between **−45 and −60 dBm**, `snr` ≥ 10 dB.

| Reading | Meaning |
|---|---|
| −45 to −60, snr 10+ | healthy |
| above −35 | overdriven receiver; large frames drop while keepalives pass. Separate the boards |
| below −90, snr low | range or antenna |
| high rssi, collapsed snr | saturation, not range — the two faults look identical on rssi alone |

Calibration note: a 119 B command and its reply both survived −17 dBm on 2026-07-20, so
saturation is real but not absolute. Treat −35 as the caution line, not a cliff.

---

## Stage E — Host ingest

```powershell
$env:RUST_LOG="warn,oh_ben_claw::spine::lora_gateway=debug"
cargo run --features hardware -- start --config bench-config.toml --no-spine `
  --provider ollama --model qwen3-coder:30b --session acceptance
```

**Pass:** within 30 s,

```
LoRa gateway → world memory node=obc-esp32-s3-001 msg=beacon rssi=-4x
```

The filtered `RUST_LOG` is deliberate — at full debug the System 2 escalation text buries
the frames you are reading.

---

## Stage F — Command path and Track 0

Grant, then the off-list pin. **The session grant dies with the brain** — re-POST after
every restart.

```powershell
$h = @{ Authorization = "Bearer bench-read-token"; "X-OBC-Operate" = "bench-operate-token"; "Content-Type" = "application/json" }
Invoke-RestMethod -Method Post -Uri "http://localhost:8080/api/v1/approvals/mesh_command" -Headers $h -Body '{"decision":"session"}'
Invoke-RestMethod -Method Post -Uri "http://localhost:8080/api/v1/tools/mesh_command" -Headers $h `
  -Body '{"node_id":"obc-esp32-s3-001","command":"gpio_write","args":{"pin":99,"value":1}}'
```

**Pass** — the refusal returns over the air:

```
SPINE ◄ src=40 seq=80 rssi=-17 dBm snr=12 dB : {"error":"safety: pin 9...
LoRa gateway → world memory node=obc-esp32-s3-001 msg=cmd_result
```

Then confirm it was scored as *health*, not fault:

```powershell
(Invoke-RestMethod -Method Post -Uri "http://localhost:8080/api/v1/tools/mesh_status" -Headers $h -Body '{}').output
```

**Pass:** `last_cmd_ok: true`, `escalated: false`, `summary.degraded: 0`.

This is the whole point of the stack: a node refusing an unsafe command on its own gate is
the system working, and the fleet view must say so.

---

## Stage G — Framing under load

```powershell
$u = "http://localhost:8080/api/v1/tools/mesh_command"
Invoke-RestMethod -Method Post -Uri $u -Headers $h -Body '{"node_id":"obc-esp32-s3-001","command":"capabilities","args":{}}'
Invoke-RestMethod -Method Post -Uri $u -Headers $h -Body '{"node_id":"obc-esp32-s3-001","command":"gpio_write","args":{"pin":99,"value":1}}'
```

**Pass:**

```
command written to base console (101 B)
command written to base console (117 B)
SPINE ► (console) seq=N   (103 B) {"args":{},"cmd":"capabilities",...
SPINE ► (console) seq=N+1 (119 B) {"args":{"pin":99,"value":1},...
```

Two writes, two frames, each starting at `{"args"`, byte counts = written + 2 (3-byte
header, minus the newline). **Fail** looks like fragments starting mid-string with counts
that don't add up — the 2026-07-19 signature was 176 B in, 81 B out.

> Known gap: sequential REST calls land ~500 ms apart, an order of magnitude wider than the
> 50 ms host gap. The sub-millisecond case (two commands enqueued in one drain pass, as
> System 2 issues them) is still unexercised. This stage proves normal pacing only.

---

## Stage H — CRC diagnostic (capability check)

Deliberately violate stage D: move base and bridge close together.

**Pass:** `SX1262 RX: CRC error ... (rssi=-8 dBm)` on the base console.

Restore separation afterwards and re-run stage D before trusting anything else.

---

## Stage I — True three-hop relay (capability check)

Relay **on**. Move the base out of the bridge's direct range — different room, wall
between. Traffic must arrive only via `gw-90`.

**Pass:** node frames reach the brain, and the base's log shows them arriving with the
relay in the path rather than direct.

**The trap:** confirm the direct path is genuinely dead first, by powering the relay off
and observing silence. Otherwise you have proved nothing about relaying — you have two
radios that can still hear each other.

---

## Diagnostic principles

Bought with time on this bench. Each one cost hours.

1. **Continuity proves a DC path, not signal integrity.** A partly-broken strand passes a
   multimeter and fails at 115200 baud.
2. **A refused command tells you about permissions, not about the world.** It is not
   evidence the node is unhealthy.
3. **Ports re-enumerate; node ids do not.** Identity comes from the boot banner, never
   from inferring which board sent what.
4. **An unexpected third radio invalidates every conclusion.** Keep the relay off unless
   stage I says otherwise.
5. **The base cannot move to battery.** It is the host's serial link, not just a radio.
   To separate radios, move the bridge.
6. **A directly-wired Heltec de-dups its own echo** and logs no `SPINE ◄` for the frame it
   just sent — the host sees silence from a healthy node.
7. **Test one layer at a time.** Whole-chain tests fail identically at every layer.

---

## Sign-off

| Stage | Result | Notes |
|---|---|---|
| A Identity | ☐ | |
| B Loopback | ☐ | |
| C Uplink | ☐ | |
| D RF baseline | ☐ | |
| E Ingest | ☐ | |
| F Command + Track 0 | ☐ | |
| G Framing | ☐ | |
| H CRC | ☐ | |
| I Relay | ☐ | |

A–G green is the trust gate: the mesh can be relied on while attention is elsewhere.
Record the date and the RSSI seen at stage D — drift in that number between runs is the
earliest sign the bench has moved.
