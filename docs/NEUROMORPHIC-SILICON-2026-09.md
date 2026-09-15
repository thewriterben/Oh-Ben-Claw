# Neuromorphic silicon — September 2026

A parts survey, not a plan. It exists to close open item 4 of
`NEUROMORPHIC-2026-09.md`, which said every hardware name in that document's
rung 3 was from memory and none had been checked. They are checked now.

Companion to `OBC-Prime/docs/EDGE-LM-2026-09.md`, which covers language models
on a node and says nothing about sensors or spiking parts. Same rules: every
number is cited, vendor claims are labelled as vendor claims, and anything
inferred rather than read is said to be inferred.

Filed in this repository rather than beside `EDGE-LM` because the ladder it
serves (`NEUROMORPHIC-2026-09.md` §5) lives here.

---

## The answer, up front

**Buy nothing yet — but for a different reason than last week.** The blocker is
no longer that the landscape was unknown. It is known, and it says three things:

1. **Exactly one part in this survey is purchasable at a published retail price
   and documented against a microcontroller today**: the Prophesee GenX320
   event sensor, sold as an OpenMV camera module at **$300** [1][2], with a
   MicroPython driver whose documentation is the most technically candid source
   in this whole survey [3]. Everything else that senses or spikes is
   quote-only.
2. **Neuromorphic *compute* is not in the same state as neuromorphic
   *sensing*.** BrainChip is the only vendor publishing list prices, and its
   parts are PCIe/M.2 cards for a host, not for a node [4]. Innatera's Pulsar —
   the only true neuromorphic microcontroller — has no public price, no public
   store, and no public datasheet [5][6][7]. SynSense Speck, the part whose
   shape matches ClawCam almost exactly, is contact-sales with a dev kit that
   wants Ubuntu 18.04/20.04 [8]. Intel Loihi 2 is research-only and **Lava was
   archived on 2026-05-13** [9] — §5 rung 3's "almost certainly out of scope"
   was right and is now a fact rather than a guess.
3. **The determinism constraint named in §4 is worse than §4 thought, and it
   binds at the *sensor*, before any spiking silicon is involved.** See §3
   below. This is the finding that should change the design, and it is the only
   thing here that changes a decision.

---

## 1. What can actually be bought

| Part | What it is | Price | Availability | Node-attachable? |
|---|---|---|---|---|
| Prophesee **GenX320** (OpenMV module) | 320×320 event sensor on a module for OpenMV H7 Plus / RT1062 / N6 | **$300** [1][2] | In stock, retail [2] | **Yes, demonstrated** — MCU host, MicroPython driver [3] |
| Prophesee **GenX320** (STM32 kit, `PAKX320ESOM2STM32`) | CM2 optical module + STM32F746G Discovery (board not included) | Not published | Vendor order code [10] | **Yes** — I²C control, CPI parallel into STM32 DCMI [10] |
| Prophesee **GenX320** (Raspberry Pi 5 starter kit) | Lensed board + 20 cm flex, MIPI CSI-2 | Not published; quote only [11] | Shipping since 2025-08 [11] | Pi-class, not MCU-class |
| Prophesee **GenX320** (EVK3, bare die, CM2/CN1/CN3/4×5 modules) | Evaluation and production options | Not published [12] | Listed, contact sales | Depends on variant |
| BrainChip **AKD1500 M.2** | Digital SNN/CNN accelerator card | **$129.00** [4] | In stock [4] | No — M.2 host card |
| BrainChip **AKD1000 M.2 / PCIe** | Digital accelerator | **$249.00 / $289.00** [4] | In stock [4] | No |
| BrainChip **AKD1000 Raspberry Pi 4 / Pi 5 dev kits** | Complete kits | **$995 / $1,495** [4] | Pi 4 low stock; Pi 5 in stock [4] | Pi-class |
| BrainChip **Brainboard 1500** | Module | **$99.00**, sold via a third party [4] | Listed, no stock state [4] | Unknown |
| SynSense **Speck** dev kit | DVS + SNN processor in one package, USB HDK | Not published [8] | Contact sales [8] | No — USB to a Ubuntu host |
| Innatera **Pulsar** | RISC-V MCU + analogue SNN + digital SNN + CNN + FFT | Not published [5][6][7] | Volume production stated for end of 2025 [6]; deployments shown at CES 2026 [13] | **In principle yes** — it *is* the MCU |
| Intel **Loihi 2** | Research chip | n/a | INRC members only; not sold [9] | No |

Two numbers worth keeping separate from the marketing. The GenX320's headline
**36 µW** is the ultra-low-power *passive* mode, in which the pixel array is
**3×3**, not 320×320 [10]. Streaming costs **3 mW** (CPI at 10 MHz, 100 kEPS–1
MEPS) rising to **22.8 mW** (MIPI at 800 MHz, 10 MEPS), and Prophesee's own
footnote says that is the sensor alone, not the system [10]. §4 of
`NEUROMORPHIC-2026-09.md` argued the case for event sensing is duty cycle
rather than power; that argument survives this survey intact, and the 36 µW
figure should never be quoted for a sensor that is watching anything.

## 2. Toolchains

| Family | SDK | License / access | Note |
|---|---|---|---|
| Prophesee | Metavision SDK (dev licence; commercial licence separate) [14]; **OpenEB** open-source core [11]; OpenMV MicroPython driver [3] | Dev licence for evaluation; commercial deployment is a separate licence [14] | The OpenMV path is the only one that runs on an MCU without a Prophesee licence conversation |
| BrainChip | MetaTF / Akida docs; model zoo [4] | Public docs | Also sells cloud access ($250/day, $995/week) [4] |
| SynSense | Rockpool, Sinabs, SAMNA [8] | Open-source toolchain, per vendor [8] | Dev kit requires Ubuntu 18.04/20.04, USB 3.0, a GPU [8] |
| Innatera | Talamo SDK, PyTorch extension for SNNs, TensorFlow-compatible [6][7] | Not public; sampling customers only [6] | Vendor claim: "no longer need a neuromorphic Ph.D." [6] |
| Intel | Lava | **Archived read-only 2026-05-13**; last release 0.10.0, Aug 2024 [9] | Intel says a new SDK for a next-generation Loihi is coming; nothing shipped [9] |

The Lava archive is the single most useful fact in this table. A framework
going read-only after four years, with the hardware still unpurchasable, is the
clearest available statement about where research neuromorphic computing sits
relative to a project that has to ship.

## 3. Determinism — the finding that matters

`NEUROMORPHIC-2026-09.md` §4 ended with a constraint: any node-side spiking
policy must be a fixed-point, fixed-timestep network, because the project
requires reproducible output, and a network relying on analogue dynamics or
hardware asynchrony cannot give it. That constraint was aimed at the
*processor*. It was aimed one tier too late.

**The sensor is where determinism dies.** Each GenX320 pixel is an analogue
front end with five configurable biases — two comparator contrast thresholds
(`DIFF_ON`, `DIFF_OFF`), a low-pass corner (`FO`), a high-pass corner (`HPF`)
and a refractory period (`REFR`) — set as DAC values [3]. Three details from
OpenMV's driver documentation, which is a primary source written by the people
who had to make the part behave:

- The driver does **not** apply the datasheet defaults on reset. It applies a
  `LOW_NOISE` preset, "because the datasheet defaults emit a much higher
  background event rate" [3]. The shipped behaviour of the part is a tuning
  choice, and a different integrator makes a different one.
- Hot-pixel calibration builds a **per-part** 320×320 hit count against a
  static scene and disables every pixel above `mean + σ·stddev` [3]. Which
  pixels get disabled is a property of the individual die and of the scene it
  happened to be pointed at during calibration.
- The on-chip spatio-temporal contrast filter exists because "pixel mismatch
  and analog noise produce extra events around the genuine transition" [3].
  Redundant events are the normal case, not a fault.

So: the same scene, the same part, twice, does not produce the same event
stream. Two parts differ more. A warm part differs from a cold one. **No
event-sensing tier can be byte-reproducible, and no amount of care in the
processor recovers it.**

That does not kill the tier. It relocates the invariant. What can be
deterministic is *the function from a recorded event stream to a posture
level* — not the stream. Which means the shape of any rung-3 work is fixed
before the hardware is chosen:

> Record the event stream to disk with provenance (part serial, bias preset,
> disabled-pixel map, timestamp base), and make the policy replayable against
> the recording, bit for bit. The sensor is an input, not a component of the
> system under test.

That is structurally the same move `TrajectoryStore` already makes for
episodes, and `perceive()` (`cce0d88`) already makes for percepts. It is worth
writing down now because it is cheap to design in and expensive to retrofit.

**On the compute side, the vendor claims do not agree with each other.**
Innatera's materials describe a "deterministic analog architecture" [15], while
its CEO told EE Times that the analogue fabric suits fast-moving signals
because "analog states cannot be maintained indefinitely, even with buffering",
and that the digital fabric's underlying clock carries a processing style that
is "completely asynchronous" [6]. Those are not the same claim. Whatever
"deterministic" means there, it is not the property this project needs, and it
would have to be established on a bench rather than read off a page. Speck is
likewise a mixed-signal part with an asynchronous DVS on the same die [8].

**BrainChip Akida is the only family in this survey that is plainly digital and
synchronous** — and it is the one whose parts do not go on a node. That is not
a coincidence; it is the trade §4 predicted, now with part numbers on it.

## 4. Could any of this reach an OBC node?

The bench nodes are ESP32-S3 Heltec LoRa boards. Concretely:

- **GenX320 → STM32F746 over CPI/DCMI is shipping today** as a Prophesee order
  code, at 10 MHz and 1 MEPS, with the sensor at 3 mW [10]. That is a Cortex-M7
  at MCU scale, so "an event sensor needs a Linux host" is false.
- **GenX320 → ESP32-S3 is plausible and unverified.** The S3's `LCD_CAM`
  peripheral takes an 8/16-bit parallel camera slave, which is the same class
  of interface as DCMI. Nobody in this survey has done it and I have not read
  the S3 technical reference manual against the GenX320 CPI timing.
  `TODO(source)`. Until someone does, treat it as an open question, not a plan.
- **Not on a Heltec.** The LoRa boards have no camera connector, and the PSRAM
  and pin budget are already committed. An event-sensing node is a different
  board, which means a different provisioning, authentication (SPINE-AUTH) and
  replay (SPINE-REPLAY) story — the same warning `EDGE-LM` §6 gives about the
  ESP32-P4.
- **Innatera Pulsar added a camera parallel interface** over the T1 [6], which
  is exactly the GenX320 CPI shape, and it is itself the MCU. If a sensor→slot
  map ever wants dedicated silicon, that pairing is the one to price. It cannot
  be priced today.

## 5. What would have to be true before any of this is bought

Unchanged from `NEUROMORPHIC-2026-09.md` §5 and worth restating because a parts
list invites shopping:

1. **Something has to perceive first.** As of today nothing does: `perceive()`
   is built and wired to `[[perception.polls]] perceive = true`, and no poll
   sets it. Rung 2 is open for want of a subject.
2. **Rung 1 step B has to produce a graded descending signal**, or an event
   sensor is feeding a one-bit output and the data rate is irrelevant.
3. **A metric has to exist.** `posture_real_effect` measured N=1 on M3. Buying
   a $300 sensor to improve a number nobody can compute is how a project
   acquires a drawer of boards.

Only after those does the question "which part" become answerable, and by then
this document will need re-checking anyway — three of the five vendors here
shipped or renamed something in the last twelve months.

## 6. What I could not verify

- **No full datasheet was read for any part.** Prophesee's GenX320 datasheet is
  behind a registration/licence wall; the numbers above come from product
  briefs and from OpenMV's driver documentation. SynSense's Speck datasheet
  links found were dated 2023 and were not opened. Innatera publishes no
  datasheet at all.
- **No price was obtained for any Prophesee product except the OpenMV module.**
  The EVK3, STM32 kit, Pi 5 starter kit and camera modules are all quote-only
  through distributors [11][12]. The $300 figure is for a module that also
  requires an OpenMV camera to be useful, which is a second purchase not
  costed here.
- **Innatera's and SynSense's availability is reported, not tested.** "Volume
  production by end of year" [6] is a statement made in June 2025 about 2025; I
  found no independent confirmation that it happened, and no store.
- **The ESP32-S3 `LCD_CAM` claim in §4 is an inference from peripheral class**,
  not a read of either the S3 reference manual or the GenX320 CPI timing.
- **BrainChip's prices were read from the vendor's own store on 2026-09-15**
  and were not compared against DigiKey, which the vendor says also stocks them.
- Nothing here was benched. This is a reading exercise, and the only
  measurement in the whole neuromorphic thread remains the one in
  `NEUROMORPHIC-2026-09.md` §1.

---

## Sources

1. LinuxGizmos / Hackster coverage of the OpenMV GENX320 module launch, Feb 2025 — https://linuxgizmos.com/openmv-introduces-the-genx320-camera-module-for-event-based-vision/. Secondary, for the $300 launch price.
2. OpenMV, *GENX320 Event Camera Module* product page and "The GENX320 is now in stock!" — https://openmv.io/products/genx320-camera-module. Primary (vendor store).
3. OpenMV, *GENX320 Event Camera*, MicroPython docs v1.28, built 2026-08-03 — https://docs.openmv.io/dev/openmvcam/sensors/genx320.html. **Primary, and the best source in this survey**: bias presets, hot-pixel calibration, STC filter rationale, raw event record layout.
4. BrainChip Inc. store, all products — https://shop.brainchipinc.com/collections/all (retrieved 2026-09-15). Primary; prices and stock states read directly.
5. Open Neuromorphic, *Pulsar — Innatera* hardware guide entry — https://open-neuromorphic.org/neuromorphic-computing/hardware/pulsar-by-innatera/. Secondary, community-maintained.
6. S. Ward-Foxton, "Innatera Adds More Accelerators to Spiking Microcontroller", EE Times, 2025-06-11 — https://www.eetimes.com/innatera-adds-more-accelerators-to-spiking-microcontroller/. Secondary, but the only source with architectural detail and direct quotes from Innatera's CEO.
7. Innatera, *Pulsar* product page — https://innatera.com/pulsar. Primary (vendor); no specifications, no price.
8. SynSense, *Speck™: Event-Driven Neuromorphic Vision SoC* — https://www.synsense.ai/products/speck-2/ (page modified 2026-01-28). Primary (vendor); dev-kit requirements read from it. Note the site now also fronts iniVation.
9. `lava-nc/lava` GitHub repository — https://github.com/lava-nc/lava. Primary. Banner: "This repository was archived by the owner on May 13, 2026." README: Loihi 1 and 2 "not available commercially", INRC membership required; latest release 0.10.0, 2024-08-08.
10. Prophesee, *GENX320 Kit for STM32* product brief (PDF) — https://www.prophesee.ai/wp-content/uploads/2023/10/GENX320ES-Product-Brief-2023-STM32-CM2-OK.pdf. Primary (vendor); source of the I²C+DCMI interface detail and the five-mode power table.
11. 1stVision, *Prophesee GenX320 Starter Kit for Raspberry Pi 5* — https://www.1stvision.com/cameras/GenX320-Starter-Kit-for-Rasberry-Pi-5. Distributor; quote-only, MIPI CSI-2 and OpenEB detail.
12. Prophesee, *Buy Products* — https://www.prophesee.ai/buy-event-based-products/ (modified 2026-09-03). Primary (vendor); confirms the product range and the absence of published prices.
13. Innatera, "Redefining the Cutting Edge: Innatera Debuts Real-World Neuromorphic Edge AI at CES 2026" — https://www.innatera.com/newsroom/redefining-the-cutting-edge-innatera-debuts-real-world-neuromorphic-edge-ai-at-ces-2026/. Primary (vendor press).
14. Prophesee, *GENX320* sensor page Q&A — https://www.prophesee.ai/event-based-sensor-genx320/. Primary (vendor): "development license… Commercial deployment is available through a separate commercial license."
15. Open Neuromorphic, *Spiking Neural Processor T1 — Innatera* — https://open-neuromorphic.org/neuromorphic-computing/hardware/snp-by-innatera/. Secondary; source of the "deterministic analog" phrasing attributed to Innatera.

Not surveyed, and why: SpiNNaker2 / SpiNNcloud (rack-scale, wrong tier);
iniVation DVXplorer cameras (USB machine-vision cameras, Linux host, and the
company is now behind the same front door as SynSense [8]); academic parts with
no commercial channel. Press coverage consulted but not relied on for any
number: Tom's Hardware on the Akida PCIe board, Hackster on the Pi 5 starter
kit, Engineering.com and PR Newswire on Pulsar.
