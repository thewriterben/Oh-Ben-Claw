# Bench Pinout Cards — MVB Boards

One card per Minimum-Viable-Bench board, focused on the pins **this project's firmware
actually drives** (for soldering/probing the bench), plus power/USB/boot and which pins are
free vs reserved. Companion to `BENCH-TEST-HARDWARE.md` and `BENCH-MVB-WIRING.svg`.

**Source tags:** `[fw]` = assigned in our firmware (file cited) · `[board]` = vendor board
reference (fixed by the board, not our code). **Always confirm against the vendor pinout /
silkscreen before soldering** — GPIO↔header mapping and safe pins vary by board revision.

---

## Card 0 — Which board is which

The three Heltecs are physically identical. Every other doc refers to them by *role*
(`heltec-base`, `heltec-gw`, `heltec-relay`), but the logs never say that — the firmware
derives its id from the MAC (`node = mac[5]`, `main.rs`) and reports `gw-40`, `gw-90`,
`gw-D8`. Nobody wrote the mapping down, and on 2026-07-19 that cost four separate
detours: the XIAO was assumed to be on the bridge when it was on the base, a jumper swap
silently reversed the working leg, the base's identity was inferred wrongly from traffic,
and two docs pointed at a COM port belonging to the XIAO.

Node ids are MAC-derived, so they are permanent per board. Roles are tape. **When the two
disagree, believe the node id.**

⚠ **Re-measured 2026-09-15: base and bridge are the other way round.** The July table
below had `gw-D8` as the base on COM3 and `gw-40` as the field bridge. Both are wrong
now — whether the boards were swapped since or the table was always wrong, the
measurement wins.

| Role | Node id | Port | Power | Wiring |
|---|---|---|---|---|
| base (host link) | **gw-40** | **COM3** (2026-09-25) | PC USB — *must* stay on the host | none — see below |
| bridge (field) | **gw-D8** | **COM12** (2026-09-25; USB on the bench; wall/bank in the field) | wall or power bank | **the node jumper pair belongs here** |
| relay (Stage 3b) | **gw-90** | — | USB power only | none — radio only |
| node | `obc-esp32-s3-001` | **COM6** | USB or bank | jumpers to the **bridge** = `gw-D8` |
| camera node | `obc-esp32-s3-002` | **COM8** | USB | **no radio yet** — spine UART unwired |
| camera node (Lilygo) | `obc-esp32-s3-003` | COM10 when plugged | USB | **no radio yet** - LILYGO T-CameraPlus-S3 V1.1, OV5640 |
| spare | `obc-esp32-s3-004` | not measured - read the banner | USB | none - spare XIAO ESP32S3, MAC 64:E8:33:7F:84:CC |
| camera node (Sense #2) | `obc-esp32-s3-005` | COM11 when plugged | USB | **no radio yet** - XIAO ESP32S3 Sense, OV3660, MAC AC:27:6E:A8:4D:E4 |

How each row was established, so the next person can redo it in two minutes rather
than infer it from traffic (which is what Card 0 exists to stop):

- **`gw-D8` is on COM5** — `espflash flash --port COM5` printed
  `MAC address: 3c:0f:02:ee:82:d8`. The MAC is the id (`node = mac[5]`), so this is
  the hardware speaking, not a role or a label.
- **COM3 is therefore `gw-40`** — the brain, plugged into COM3, ingests
  `node=gw-D8 msg=gw_keepalive`. A station never logs a `SPINE ◄` for its own
  frames, so the station hearing `gw-D8` cannot be `gw-D8`. With `gw-90` unpowered,
  COM3 is `gw-40`.
- **The node is currently jumpered to `gw-40`** — frames carrying the node's beacon
  arrive at `gw-D8` as `src=40`, and a relay preserves the original `src`, so
  `gw-40` originated them. That is the wrong board: see the jumper rule below.

All three radios self-test clean (`status=0xA2`, syncword readback `0x1424`).

⚠ **Ports re-measured 2026-09-17, and they had moved again.** The table above said
`gw-40` on COM3 and `gw-D8` on COM5; both boot banners were read directly that day and
say otherwise — COM3 prints `Gateway D8`, COM4 prints `Gateway 40`, and COM5 is not
present at all. The live config agrees (`station = `gw-40`, `port = `COM4`). This is the
second correction to this table; read the banner, never the table, before flashing.

### `no-relay` on gw-40, measured 2026-09-17 — the flag works, the benefit does not reproduce

Each station's build is now recorded here, because the 7b restore showed that
guessing them is a way to silently strip a feature:

| board | port | features |
|---|---|---|
| `gw-D8` | COM12 | `bench-low-power` |
| `gw-40` | COM3 | `bench-low-power`, `bench-nvs-fault`, **`no-relay`** (2026-09-17) |

`no-relay` was enabled on the base station because it is the sink and the
firmware comment says a station with a host plugged in should not play relay.
The comment also carries a number, measured 2026-09-12: flood-relaying "cost it
about half of everything gw-40 sent — the frame after any received frame,
reliably."

**That number did not reproduce, and the experiment that went looking for it was
broken by construction.** Three minutes of gw-40's console either side of the
flash, counting gaps in gw-D8's `seq`:

```
BEFORE  seq 80..114   span 35   received 31   missed 4   delivery 88.6%   relays 31
AFTER   seq 145..173  span 29   received 23   missed 6   delivery 79.3%   relays  0
```

The flag does exactly what it says: **31 relays → 0**. Delivery did not improve;
it read worse, and on 4-versus-6 misses over ~30 frames that difference is not
distinguishable from noise either way.

**The tautology worth recording.** The BEFORE run also reported "4 of 4 missing
seq directly follow a relayed frame (100%)", and that looked like the mechanism
caught red-handed. It is not evidence of anything. gw-40 relayed **every frame it
received** — 31 relays against 31 receptions — so *any* miss necessarily followed
a relay. The statistic could not have come out otherwise, whatever the cause of
the loss. A correlation with a saturated control variable measures the control,
not the effect.

So the honest position: relaying is gone, which is architecturally right and free;
the delivery claim from 2026-09-12 is neither confirmed nor refuted here; and
there is a baseline loss source of roughly 10-20% that relaying does not explain,
because removing relaying did not remove it. That is the thing worth chasing, and
it wants a longer run with the counting done on gap *rate* over many minutes
rather than two three-minute samples.

### The baseline-loss run (2026-09-25): the base was talking over its own traffic

Two samples of about 30 frames cannot support "10-20%". The 95% intervals are
4.5-26% for 4/35 and 9.8-38% for 6/29. `relay_loss.py` also watched only the
receiver, and it counted the one-byte `seq`, which wraps after 255 frames. So
the run was redone as `scripts/mesh_loss.py`. It watches **both** consoles and
gives every frame gw-D8 logs as sent exactly one fate: `received`, `rejected`,
`crc`, `header`, `deaf` (gw-40 was transmitting) or `silent`.

**Why gw-40 is reflashed partway through.** Until 2026-09-25 the SX1262's
IRQ mask left out CrcErr and HeaderErr. A masked IRQ never latches, so the
"CRC error" warning could not fire, and none has ever been logged.
- A CRC-failed frame was read out as good and died as `REJECTED … bad tag`.
- A frame with a corrupt header vanished without a line.

**Ports, a third correction.** Read by `espflash board-info` on 2026-09-25:
COM3 is gw-40 (`3c:0f:02:ee:83:40`) and COM12 is gw-D8 (`3c:0f:02:ee:82:d8`).
The table above is updated. Read the MAC, never the table.

**Run A: the old firmware, 45 minutes, 545 frames.**

```
loss 50/545 = 9.2%  (95% CI 7.0-11.9%)
fates: received 495, rejected 0, crc 0, header 0, deaf 50, silent 0
keepalive 41/456 9.0%   uart 9/89 10.1%   (no size effect)
5-min bins: 17/61 7/60 3/60 0/61 2/60 0/61 2/60 8/61 11/61
rssi median -38 dBm, snr 12
```

- **Every loss was `deaf`.** All 50 missing frames were on the air while gw-40
  was transmitting, which would put about 2 of the 50 there by chance. A
  half-duplex radio cannot hear while it talks.
- **Zero `silent`**, so the old IRQ mask was not hiding a pile of corrupt
  frames. Its fix is still right, but it recovers none of this loss.
- **No size effect**, so this is not the link: long frames did not die first.
- **Bursty.** 17 of 61 lost in the first five minutes, then none for long
  stretches, then 11 of 61 in the last five. The two stations' keepalive
  schedules drift in and out of step, and each stretch in step is a run of
  collisions. That is why the old two-sample "10-20%" and "about half"
  disagreed with each other: they sampled different phases.

**The fix, in both directions** (heltec `main.rs` and `sx1262.rs`):
- The base (`no-relay`) keepalive goes from 5 s to 30 s. Nothing waits on
  it: the node excludes keepalives from host-link liveness, and the host
  cannot hear the station it is plugged into.
- Every station's keepalive now listens before it talks
  (`Sx1262::channel_busy`). It answers "busy" if a frame is already arriving
  (HeaderValid, checked without leaving RX) or if a 2-symbol CAD hears LoRa.
  A busy channel defers the keepalive 300-700 ms; after 5 deferrals it goes
  anyway.
- The console command hold-off now only ever delays a keepalive, rather than
  pulling a 30 s one forward to 3 s.

**Run C: both stations on the new firmware.** gw-D8 needs it too, for its own
listen-before-talk. Both builds also carry the CRC and header IRQ fix, so
Run C replaces the planned Run B.

```powershell
# stop the brain first. Read both MACs (espflash board-info) before trusting ports.
. $env:USERPROFILE\export-esp.ps1; $env:CARGO_TARGET_DIR='C:\e'
$env:OBC_SPINE_ROOT = (Get-Content $env:USERPROFILE\.obc\spine_root)
cd firmware\heltec-lora-linktest
cargo build --release --features bench-low-power,bench-nvs-fault,no-relay   # gw-40
espflash flash --port COM3 C:\e\xtensa-esp32s3-espidf\release\heltec-lora-linktest
cargo build --release --features bench-low-power                            # gw-D8
espflash flash --port COM12 C:\e\xtensa-esp32s3-espidf\release\heltec-lora-linktest
cd ..\..
python scripts\mesh_loss.py --tx COM12 --rx COM3 --minutes 45 --label "C: base 30 s + LBT"
```

Success looks like `deaf` falling from 9.2% toward zero and staying there in
every 5-minute bin. The report counts listen-before-talk deferrals on each
side. A high `forced` count would mean the channel was busier than the
backoff allows for.

⚠ **After a plain `espflash flash` (no `--monitor`), gw-D8 may not start.**
On 2026-09-25 it stayed in the ROM downloader and printed nothing for a whole
45-minute run, while gw-40 flashed the same way came up normally. A later
`espflash monitor` connected through the flash stub and still showed no boot
until Ctrl+R hard-reset it. Before starting any capture, open each station
with `espflash monitor --port <port>`, press Ctrl+R, and see keepalives
before you press Ctrl+C.

**Run C: base 30 s + listen-before-talk, 45 minutes, 542 frames** (both
stations on commit 2a54d08; gw-D8 ELF `fe1e72eb4`):

```
loss 8/542 = 1.5%  (95% CI 0.7-2.9%)       Run A: 9.2% (7.0-11.9%)
fates: received 534, deaf 2, header 1, silent 5, rejected 0, crc 0
keepalive 8/453 1.8%   uart 0/89 0.0%
LBT: gw-D8 deferred 4 / forced 0;  gw-40 deferred 8 / forced 0
5-min bins: 1/61 0/60 2/60 0/61 0/60 3/61 0/60 1/61 1/58
rssi median -40 dBm, snr 13
```

- **Collisions fell from 50 to 2.** The two 95% intervals do not overlap.
  The 12 deferrals are collisions that did not happen. The 2 left are frames
  that started within the same CAD window, which listen-before-talk cannot
  see.
- **No bursts.** The worst bin is 3/61, where Run A's was 17/61.
- **`header: 1` is the first header error ever logged on this mesh.** The
  IRQ-mask fix works on hardware. Before it, that frame would have been
  `silent`.
- **The new floor is `silent`, 5 keepalives (0.9%).** These are frames
  gw-40 recorded no trace of. Candidates, all untested: arrival during
  gw-40's own CAD or TX setup, or a preamble too weak to trigger a header.
  At this rate, telling them apart needs runs of several hours.

Each run writes `results/mesh_loss-<stamp>.log` (the raw capture from both
consoles) and a `.json` next to it. `--replay <log>` re-analyses a capture
without the hardware. Keep node `obc-esp32-s3-001` jumpered and reporting as
usual, so node reports (about 220 B, 348 ms on air) and keepalives (about
66 B, 123 ms) are both in the sample. Loss is broken down by frame size
because long frames have failed alone before. Note whether gw-90 is powered:
its relayed copies of gw-D8's frames count as delivered.

### ⚠ Before you flash any ESP32-S3, run the gate

```powershell
powershell -ExecutionPolicy Bypass -File scripts\which_esp32.ps1
```

It names every attached S3 by its factory MAC and tells you which port is safe.
Then pass that port explicitly — `espflash --port COM8` — and never let espflash
autodetect while the live node is plugged in.

**This is not paranoia, it is the 2026-09-16 near-miss.** Both XIAOs are the same
board, from the same batch, and their MACs differ only in the last three bytes
(`…7E:BB:98` vs `…7E:7E:04`). The plan that day said "flash the spare XIAO"; the
only S3 attached at the time was the live mesh node. Nothing on the desk, in the
port list, or in the firmware distinguished them.

Then it got worse before it got better: the second board, once flashed, booted
announcing `Node ID: obc-esp32-s3-001` — the live node's identity — and began
emitting `link_state` JSON under it. The node id was a compile-time constant.
Nothing reached the air only because that board's spine UART was not yet wired to
a radio, and wiring it to one was the next step in the plan.

Identity now comes from the chip (`firmware/obc-esp32-s3/src/identity_map.rs`),
the boot log prints the MAC beside the name, and
`tests/firmware_identity_roster.rs` fails if this card, the firmware roster, the
host registry and the gate script ever drift apart.

**Keep the relay unpowered outside Stage 3b.** On 2026-07-19 a frame carrying a
host-originated command was observed with `src=90` — the relay — which means all three
radios were live and the topology under test was a three-radio flood, not the two-radio
path everyone was reasoning about. Frame paths that "don't add up" are usually this.

To re-confirm identity after any swap, read the banner rather than inferring from
traffic (`main.rs`):

```
Gateway 40 — UART1(TX=4,RX=2) ⇄ LoRa. Wire compute TX→GPIO2, GND↔GND.
```

Power each board in turn, note the two hex digits, write them on tape *and* in this
table. Ten minutes once, versus inferring it wrongly every time.

⚠ **The base station `gw-D8` on COM3 was overwritten on 2026-08-22.** Node firmware
(`firmware/obc-esp32-s3`, default XIAO pin map) was flashed to it by mistake: the XIAO was
not plugged in, COM3 was the only port present, and every board on this bench is an
ESP32-S3 with 8 MB of flash, so the flash log looked exactly right. **It is not a working
base station until it is reflashed with the gateway build.** Anything in Stage 3 that
assumes a live base is currently untrue.

*Resolved 2026-09-15.* `gw-D8` was flashed twice that evening with the gateway build
(`bench-low-power,bench-wrong-root` for SPINE-REPLAY §6 step 7a, then `bench-low-power`
with the real root to restore it) and is a working station again — on **COM5**, as the
corrected table above says, not COM3. Its frame counter continued across both reflashes
(15401 → 15431), so the NVS ceiling survives a firmware change and not merely a reboot.

The cheap check that would have caught it, and that `bench_run.py` now enforces: the node
is native USB-Serial-JTAG, Espressif VID `0x303a`. The Heltecs are CP210x bridges, VID
`0x10c4`. Roles are tape and ports re-enumerate, but the USB descriptor is a property of
the hardware.

⚠ `BENCH-WALKTHROUGH.md` §3.3 and `PHASE-B-LORA-MESH.md` both showed the base on **COM6**.
That was stale — COM6 is the XIAO on this bench. Ports re-enumerate; confirm before trusting
any port in any doc. Node ids do not.

**The base cannot move to battery.** It is the brain's serial link, not just a radio. To
separate the radios, move the *bridge* (and the XIAO with it — they are jumpered together).

**TX always lands on RX.** Both jumpers are directional and swapping the pair kills both
directions at once, which looks exactly like a dead node:

```
node D6 (GPIO43, TX)  ──►  bridge GPIO2 (RX)     node → mesh
bridge GPIO4 (TX)     ──►  node D7 (GPIO44, RX)  mesh → node
node GND              ◄─►  bridge GND            common reference
```

**"bridge" is a role, and the board holding it changes.** As of 2026-09-15 that is
**`gw-D8`, on COM5** — *not* the board on COM3, which is the base and must stay bare.
This block used to name `gw-40` outright; the boards then swapped roles and the
instruction silently became the failure mode two paragraphs down. Wire by role, confirm
the id from the boot banner or the flash MAC, and only then pick up a jumper.

**Two RF facts that keep resurfacing:**

- Target keepalive RSSI **−45 to −60 dBm**. Above about −35 the receiver overdrives:
  ~55 B keepalives still pass while 120 B+ frames vanish, so the link looks healthy and
  commands silently disappear. Cost an evening on 2026-07-17 and recurred 2026-07-19.
  `snr=` in the `SPINE ◄` line separates the two cases — weak-and-clean is range,
  strong-and-dirty is saturation.

  **The band has two edges, and the weak one was measured on 2026-09-16.** The bench
  sat at a *stable* −80 (5 dB spread over 200 frames) and was tuned back to a settled
  **−51/−53**. Three things came out of it worth keeping:

  - **Distance was the smaller half of the fix.** The boards went from ~5 ft to
    under 2 ft, which at 915 MHz is only about **8 dB**; the measured gain was
    **36 dB**. The other ~28 dB came from what was corrected on the way —
    connector seating and antenna polarisation. Two λ/4 whips at right angles lose
    10–20 dB on their own. Reach for seating and orientation *before* reaching for
    distance, because distance is the part you cannot buy back in the field.
  - **Erratic and weak are different faults.** An intermittent connection reads as a
    wide swing (−69 to −101 within four minutes, boards stationary); a fixed
    attenuation reads as a *tight* spread at the wrong level. The first is a
    connector or a ground, the second is seating or geometry. Diagnose from the
    spread, not the average.
  - **RSSI is measured on beacons, which are short.** Good RSSI is necessary and not
    sufficient: both edges of the band eat long frames first. See the cold-start
    check below for the test that actually settles it.
- A Heltec **wired directly to the node** transmits that frame and then de-dups its own
  echo, so it never logs a `SPINE ◄` line for it. The host sees silence from a perfectly
  healthy node. Only the bridge should carry the jumpers.

  **Cost an evening again on 2026-09-15**, so here is the exact mechanism rather than
  the symptom. A UART-origin frame is logged by `main.rs` as
  `SPINE ► (uart) seq=… ({n} B) {payload}` — a **TX** line. The host's
  `parse_gateway_line` anchors on `SPINE ◄` and documents that it returns `None` for
  "TX lines (►), relay lines (⇒), malformed-frame notices, and boot logs". So the
  brain discards its own node's uplink by category, before the echo de-dup above even
  comes into it. The node looked dead for a whole session: beaconing every 30.78 s,
  stable `boot_id`, pinned in `deny-all` because its boot announcement never reached
  the supervisor that would push its limits back, and every `descend` unanswered
  after three attempts because the reply came home the same discarded way.

  **This is an authentication boundary, not a parser oversight — do not "fix" it in
  the host.** The `► (uart)` line carries no `ctr=` and no `mac=`, so even if the
  parser accepted it, `LoraAuth` would have nothing to verify and the host would be
  trusting a console. That is precisely the trust boundary SPINE-AUTH closed
  (`lora_gateway.rs`: "the host trusts the station's radio, not its console").
  **A node's uplink has to arrive over the air to be authenticated at all**, which is
  what makes "only the bridge carries the jumpers" a design rule rather than bench
  tidiness.

**The cold start is a free long-frame test — use it.** (2026-09-16.)

This bench is powered off overnight, so every morning the node boots fresh, and the
boot sequence exercises the one path RSSI cannot vouch for:

1. node boots with `policy: "deny-all"` and beacons it
2. the supervisor sees a boot it holds limits for and **pushes limits** — a long
   outbound frame
3. the node applies them and replies `cmd_result` `{"applied":true,…}` — a long
   inbound frame

**The attempt count on that push is a direct link-quality measurement**, and it is
free, daily, and needs nobody at the bench. At ~−70 dBm on 2026-09-16 it needed
**three** attempts (`…r1`, `…r2`); at −53 it should take **one**. A push that still
needs three on a good link means the link was never what was breaking commands.

That matters because short frames survive links that eat long ones, in *both*
directions of the band. Between cold starts the node's non-beacon frames
(`link_state`, `reflex`, `cmd_result`) are event-driven and historically arrive about
**once per 53 minutes** — so waiting for one to prove the link is not a plan, and a
quiet hour is not evidence of a fault.

Bench tools live in `C:\Users\Benji\obc-bench\` (outside any repo, so they survive):
`rssi_live.py` prints per-frame RSSI with both edges flagged, `boot_id_check.py`
shows the boot id and the limits-push attempt count, `morning_bench_report.py` is what
the 10:00 scheduled check runs.

---

## Card 1 — Heltec WiFi LoRa 32 V3  (ESP32-S3 + SX1262)

Role: **all three Station B radios** — `heltec-base`, `heltec-relay`, `heltec-gw` (field).
`firmware/heltec-lora-linktest`. Same card for all three: identical radio pinout and config.
The **relay** is radio-only — antenna + USB power, **no external wiring at all** (the UART
bridge below applies only to `heltec-gw`); it forwards frames (TTL−1, de-dup) and earns its
keep in walkthrough **Stage 3b** (true 3-hop test).

**SX1262 radio (SPI)** — `[fw]` `src/sx1262.rs`
| Signal | GPIO | | Signal | GPIO |
|---|---|---|---|---|
| NSS (CS) | 8 | | RST | 12 |
| SCK | 9 | | BUSY | 13 |
| MOSI | 10 | | DIO1 (IRQ) | 14 |
| MISO | 11 | | TCXO | via **DIO3** (1.8 V) |
| | | | RF switch | via **DIO2** |

**Phase-B UART bridge to the XIAO node** (`heltec-gw` **only** — currently `gw-D8`/COM5;
never the base) — `[fw]` `src/main.rs`, `docs/PHASE-B-LORA-MESH.md`
| Signal | Heltec GPIO | Direction |
|---|---|---|
| RX (from XIAO TX) | **GPIO2** | XIAO GPIO43 → here |
| TX (to XIAO RX) | **GPIO4** | here → XIAO GPIO44 |
| GND | GND | common ground (verify with meter) |

The banner prints this pair on every boot, so it is checkable without a meter:
`Gateway XX — UART1(TX=4,RX=2) ⇄ LoRa. Wire compute TX→GPIO2, GND↔GND.`
⚠ **UART0/GPIO43-44 on the Heltec is its own CP2102 USB console** — a different pair from
the XIAO's D6/D7, which are *that* board's GPIO43/44. Two boards, same GPIO numbers,
opposite roles; read the column headers before cutting a jumper.

**Board reference** — `[board]`
| Function | GPIO |
|---|---|
| OLED I2C SDA / SCL / RST | 17 / 18 / 21 |
| Vext power control (active-LOW, powers OLED) | 36 |
| User LED | 35 |
| ADC battery divider (VBAT) | 1 (via on-board divider, board rev dependent) |
| USB-C (CP210x on some clones / native S3 on V3) + BOOT(GPIO0) + RST | — |

⚠ **Attach the 915/868 MHz antenna before any TX.** Radio config: SF7 / BW125 / CR4-5 /
syncword 0x1424 / +22 dBm. Free GPIO for probing: avoid the SX1262 + OLED + Vext pins above.

---

## Card 2 — Seeed XIAO ESP32-S3 Sense

Role: **mesh sensor/camera node** behind the field Heltec (Station B). Runs `obc-esp32-s3`.

**UART bridge to Heltec** — `[fw]` `docs/PHASE-B-LORA-MESH.md`
| Silk | GPIO | Function |
|---|---|---|
| **D6** | **43** | TX → Heltec GPIO2 |
| **D7** | **44** | RX ← Heltec GPIO4 |
| GND | GND | common ground |

**Board reference** — `[board]` (14-pin, both sides; 3V3 / GND / 5V on the power end)
| Silk | GPIO | | Silk | GPIO |
|---|---|---|---|---|
| D0 | 1 | | D6 | 43 (UART0 TX) |
| D1 | 2 | | D7 | 44 (UART0 RX) |
| D2 | 3 | | D8 | 7 (SCK) |
| D3 | 4 | | D9 | 8 (MISO) |
| D4 | 5 (SDA) | | D10 | 9 (MOSI) |
| D5 | 6 (SCL) | | 3V3 / 5V / GND | power |

**Sense expansion board** `[board]`: OV2640 camera + PDM mic are wired to the S3 via the
Sense daughterboard (DVP + PDM), plus a microSD slot — not broken out to the 14-pin header.
Free header pins for probing: D0–D5, D8–D10 (mind D4/D5 = I2C, D8–D10 = SPI if used).

---

## Card 3 — Waveshare ESP32-S3-Touch-LCD-2.1

Role: **primary control / reflex-safing + sensing node** (Station A). Runs `obc-esp32-s3`
built with **`--features board-waveshare-21`** — the default build is the XIAO pin map and
would drive this board's LCD lines as GPIO. *(Card corrected 2026-07-16 against the
Waveshare wiki/schematic — the old card's 3/14/26/33/46 outputs and 4/5 I2C don't exist here.)*

**The board exposes exactly three connectors** — `[board]`
| Connector | Pins |
|---|---|
| 12-pin header | GND ·5V· **GPIO19/20** (native-USB D−/D+) · 3V3 · **SCL=7 / SDA=15** (I2C-only) · **TXD=43 / RXD=44** · NC · **IO0=GPIO0** |
| I2C connector | GND / 3V3 / SCL=**7** / SDA=**15** (same bus as header) |
| UART connector | GND / 3V3 / 43 / 44 — dead while the UART Type-C is plugged in |

**No silkscreen on the header** — fingerprint it: (power off) two pins have continuity to the
USB shell = GNDs at positions 1 & 5, and the end where a GND is *outermost* is the pin-1 end;
hold **BOOT** → one more pin gains GND continuity = **IO0, pin 12** (the DHT22 pin). Power on:
pin 2 ≈ 5 V, pin 6 = 3.3 V, pins 7/8 idle ≈ 3.3 V. Full steps: datasheet §fingerprint.

**Firmware-assigned (`board-waveshare-21`)** — `[fw]` `firmware/obc-esp32-s3`
| Subsystem | Pins (GPIO) |
|---|---|
| **Safe output pins** (Track-0 GPIO writes) | **43, 44** (UART1 spine uplink disabled on this build) |
| DHT22 data | **0** (header pin IO0) + 10 kΩ pull-up to 3V3 |
| I2C bus (sensors) | SDA **15** · SCL **7** (hardwired connector) |
| Command I/O | native USB-Serial-JTAG (GPIO19/20 — the "USB" Type-C) |
| Camera / I2S mic | **not possible on this board** — stubs |

**Station-A sensor hookups**
| Peripheral | Wiring |
|---|---|
| BME280 @0x76 / MPU-6050 @0x68 | I2C connector SDA=15, SCL=7 — bus already carries touch/IMU/RTC at 0x15/0x20/0x51/0x6B/0x7E; no conflict |
| DHT22 | + →3V3 · out → **IO0** (+10 kΩ→3V3, keeps BOOT strap high) · − →GND |
| LED + 330 Ω | **GPIO43** (header TXD pin) → LED → GND · `gpio_write pin 43` |

**Board reference** — `[board]` (`docs/datasheets/waveshare-esp32-s3-touch-lcd-2.1.md`):
round 480×480 RGB LCD (ST7701), CST820 touch, QMI8658 IMU + PCF85063 RTC onboard (free
extra sensors on the I2C bus!), TCA9554 expander internal-only, two Type-C ports.
⚠ The LCD consumes GPIO 1–3, 5–14, 16–18, 21, 38–41, 45–48 — never drive those as GPIO.
⚠ GPIO0 is the BOOT strap — anything on it must idle HIGH at reset (the DHT22 + pull-up does).

---

## Card 4 — Espressif ESP32-S3-EYE v2.2

Role: **ClawCam camera-trap node** (Station C). `firmware/clawcam_node_espidf`,
`boards/esp32_s3_eye_v22.json`. *Pinmap unverified until bench tests pass.*

**Camera (OV2640, DVP)** — `[fw]` `esp32_s3_eye_v22.json`
| Signal | GPIO | | Signal | GPIO |
|---|---|---|---|---|
| XCLK | 15 (16 MHz) | | D0–D7 | 11, 9, 8, 10, 12, 18, 17, 16 |
| SIOD (SDA) | 4 | | VSYNC | 6 |
| SIOC (SCL) | 5 | | HREF | 7 |
| | | | PCLK | 13 |

**Storage (microSD, SDMMC 1-bit)** — `[fw]`
| Signal | GPIO | Mount |
|---|---|---|
| D0 | 40 | `/sdcard` |
| CMD | 38 | (FATFS) |
| CLK | 39 | |

**Motion / power** — `[fw]`
| Function | Value |
|---|---|
| PIR wake (EXT0) | **unassigned** (`pir_gpio = -1`) → **wire an HC-SR501/AM312 to a free RTC-capable GPIO** |
| Battery ADC | `battery_adc_channel = -1` (battery pads only; no on-board gauge) |
| Low-battery threshold | 3.55 V |

⚠ The S3-EYE has **no built-in PIR** — the camera-trap wake path needs an external PIR on an
EXT0-capable pin. Confirm the pinmap on first bench flash (status: `unverified`).

---

## Probing quick-tips
- **Common ground first.** Every cross-board link (UART bridge, PIR, sensors) needs a shared
  GND — meter it before powering.
- **Don't probe LoRa RF pins live**; keep the antenna on.
- **Boot/flash:** ESP32-S3 boards enter download mode via BOOT(GPIO0)+RST; native-USB S3
  boards usually auto-reset with `espflash`.
- **Safe outputs only** for the Waveshare GPIO smoke test — 3/14/26/33/46 (Track-0 allow-list).
- When in doubt, cross-check the vendor pinout — these cards cover the *project-used* pins,
  not every header pin.

*Sources: `Oh-Ben-Claw/firmware/heltec-lora-linktest/src/sx1262.rs`,
`Oh-Ben-Claw/firmware/obc-esp32-s3/{BRINGUP.md,CAMERA.md}`,
`Oh-Ben-Claw/docs/{PHASE-B-LORA-MESH.md, datasheets/waveshare-esp32-s3-touch-lcd-2.1.md}`,
`ClawCam/firmware/clawcam_node_espidf/boards/esp32_s3_eye_v22.json`. Board-reference rows
are vendor pinouts — verify against current silkscreen/datasheet.*
