# The frame-difference detector, and the two mechanisms that lost

**Status:** measured on the bench 2026-09-16. Host-side only — nothing runs on a
node yet. The fixture and the check are in the tree; the firmware is not.

The road to G2 is a `CC|` summary on the air, and a summary needs something to
summarise. That was going to be "a frame-difference score". This is what
happened when it met a real room.

## The problem, which the room demonstrated unprompted

While the camera was being brought up, Benji turned on a light, moved a lamp,
repositioned the camera and leaned into frame — ordinary things, done for
ordinary reasons. Three of those produce a large whole-frame delta and **only
one is a detection**:

| event | pixels changed | is it a detection? |
|---|---|---|
| lamp switched | nearly all | no — nothing moved |
| camera nudged | nearly all | no — the world is identical |
| person in frame | many | **yes** |

A naive `mean|f - prev|` scores all three alike. On this bench the first two are
the *common* case, so a detector tuned without them reports a night of activity
that was one light switch. That is the alarm-fatigue failure the mesh
escalations already produced (`safe-mesh-node-lost` firing at Critical every
tick until the wake budget swallowed it), arriving in the vision tier.

## The fixture

`tests/fixtures/vision-bench-2026-09-16/` — 148 frames from node
`obc-esp32-s3-4b95f8` (LILYGO T-CameraPlus-S3 V1.1, OV5640, QVGA JPEG), in four
labelled sets: `baseline_still`, `light_change`, `person`, `camera_nudge`.
Captured with `scripts/vision/capture_set.py`, 40 frames at 0.75 s.

**148 frames, one bench, one afternoon, one lighting setup, one sensor. This is
a fixture, not a validation.**

## The floor, and the frame that is not noise

Baseline, 28 deltas of an empty bench:

```
raw   mean  2.62  p95  3.66  max  4.61
edge  mean  2.49  p95  3.08  max  3.40
frac  mean 0.041  p95 0.068  max 0.083
```

Brightness held 108.7–111.0 across 29 frames. Steady.

**Frame 000 is the exception and it is a finding.** Brightness 62.9, `raw` 80.6,
87% of pixels changed — 17× the steady-state max, with nothing happening. That
is auto-exposure converging after the port-open reset. A detector armed at boot
calls it a major event *every boot*. The node must withhold judgement until
brightness settles and **say it is warming up**, which is this project's
no-silent-degradation rule landing in a new place.

## Two proposed mechanisms, both refuted

Both were argued from physics before the data arrived. Both lost.

| score | idea | result |
|---|---|---|
| `norm` | subtract each frame's mean, so a uniform brightness shift cancels | **no help**: raw p50 7.65 → norm p50 7.99. Mean-subtraction cancels a uniform *additive* shift; a lamp is a multiplicative gain landing unevenly, so there was nothing to cancel. |
| `gain` | divide by each frame's mean, matching the multiplicative physics | **worse than nothing**: 2.4× the quiet floor against raw's 2.1×, and its fraction variant 3.6×. It amplified what it was meant to remove. |

The winner was not proposed in advance: **how many pixels changed** (`frac`) for
presence, and **whether structure changed** (`edge`) for what changed.
Localisation and structure, not brightness arithmetic.

## Peaks, per set

```
                 raw    frac    edge
baseline         4.6   0.083     3.4
light_change    14.7   0.309     3.3   <- edge at the floor: fully suppressed
person          66.3   0.943     7.7
camera_nudge   101.4   0.966    13.1
```

`frac` cannot tell a person from a nudge (0.943 vs 0.966). `edge` can: a
lighting change leaves structure alone, a nudge moves every edge at once.

> **A methodological error worth keeping.** The first comparison used each set's
> *median* against the baseline, and concluded the camera nudge was a non-event
> — every score read ~1.0. The nudge was a transient, one or two frames out of
> 39, so the median of that set is just the quiet room. A statistic that
> summarises a set cannot see a brief event inside it. The analysis is per-frame
> for that reason.

## The rule

Three thresholds on two cheap numbers:

```
frac ≤ 0.35              → quiet
frac high, edge ≤ 4.2    → light    (pixels changed, structure did not)
frac high, edge ≥ 9.0    → nudge    (structure changed everywhere)
otherwise                → motion   (structure changed in a patch)
```

`light` and `nudge` are **reportable states, not detections**. `det=1` for a
light switch is a lie; so is `det=1` for someone bumping the tripod. The honest
summary of a repositioned camera is "I cannot compare to before".

## Result

```
set                 quiet    light   motion    nudge     n
baseline_still         28        0        0        0    28
light_change           38        0        0        0    38
person                 15        3       20        0    38
camera_nudge           26        2        2        8    38
```

**Zero false positives across 66 frames of quiet room and lighting swing** — the
±14 grey-level oscillation that swamped every brightness-based score produces
not one detection.

### Where it is wrong

- 3 `person` frames land in `light` (edge 4.02–4.19, just under the boundary).
  Under-reporting.
- 2 `camera_nudge` frames land in `motion`, at deltas 0 and 2 — *before* the
  nudge at delta 3. Almost certainly a hand entering frame to reach the camera,
  in which case they are true motion and the **set label** is what is wrong. Not
  provable from these numbers, so it is recorded as open rather than excused.

These counts are pinned in `scripts/vision/classify.py` as `EXPECTED`, mistakes
included. Smoothing them away would hide the next change.

## The check

```
python scripts/vision/classify.py --check
```

Exits non-zero if the classifier stops matching the fixture, and separately if a
quiet room or a lighting change ever produces a detection — the property the
whole thing exists for. **Verified to fail:** raising `FRAC_HI` to 0.99 blinds
the detector and the check reports 2 regressions and exits 1. A guard nobody has
seen fail is not a guard.

Not wired into CI yet. It needs `numpy` and `Pillow`, which the host test job
does not currently install.

## What this means for the node

The detector needs `frac` and `edge`, which means **pixel access**. The camera is
configured `PIXFORMAT_JPEG`, so a node would have to decode its own JPEG to
difference it — expensive and silly.

The detector path should capture `PIXFORMAT_GRAYSCALE`: QVGA grey is 76,800
bytes, trivial against 8 MB of PSRAM, and both scores fall out of two passes over
the buffer. That means **the node runs two capture modes** — grey to decide,
JPEG only when it has something worth showing — which is a real design decision
and wants an ADR before it is written.

Nothing here has run on a node. The thresholds are measured against a host-side
decode of JPEG frames; an on-node greyscale pipeline sees slightly different
pixels (no JPEG round-trip) and the numbers must be re-measured there, not
assumed to carry over.

---

# 2026-09-17: it runs on a node, and the re-measurement has not happened yet

The detector is on `obc-esp32-s3-003` (LILYGO T-CameraPlus-S3 V1.1, OV5640),
reachable as `camera_detect`, differencing the sensor's raw Y8 with no JPEG
round trip. `scripts/probe_detect.py` drives it. The ADR for the single capture
mode is OBC-Prime `docs/DECISIONS.md`, 2026-09-17.

The rule is now implemented twice — `scripts/vision/classify.py` and
`firmware/obc-esp32-s3/src/detector_math.rs` — and
`tests/firmware_detector_math.rs` scores real frames through both and fails if
they disagree, because two copies of one rule that nothing compares is how this
tree has been bitten before.

## The first on-node run, and why it is not the measurement

Thirty frames at 0.4 s. Six of thirty were judged; the rest reported
`warming_up`. Brightness wandered 84–119 where the host fixture held 108.7–111.0,
and `frac` on frames the rule called quiet ran 0.11–0.23 against the host
fixture's baseline max of 0.083.

That looks exactly like the ADR's prediction coming true — raw Y8 carries sensor
noise that JPEG quantisation smooths away, so more pixels cross a 12-grey-level
threshold, and the floor rises. It is a tidy story and **this run is not evidence
for it.**

A picture taken during the same session shows a person at the bench, in frame,
moving. So the brightness swing and the raised `frac` are confounded with an
actual moving subject, and the run measures a room with someone in it. A floor
measured on a scene that is not quiet is not a floor.

This was nearly written up as "the thresholds do not carry over". It was caught
by looking at the image the node had just produced — which only became possible
in the same change, because the node could not encode a picture until then. The
detector's own output could not have told anyone: `warming_up` is what it says
both when auto-exposure is hunting and when the room is busy.

## What the re-measurement needs

An empty bench, nobody in frame, several minutes, lighting held still — the
conditions `baseline_still` was captured under — and then the same for a lighting
change. Until that exists:

- every `camera_detect` reply carries `thresholds_provisional: true`
- the three thresholds in `detector_math.rs` remain the host fixture's
- nothing acts on `class`

`SETTLE_DELTA` (3.0 grey levels, two consecutive frames) is in the same position:
it came from a fixture whose exposure was stable, and the only run against it so
far had a person in the frame.

## What the run does establish

- The detector executes on the node and returns per-frame `frac`, `edge` and
  `mean` — the numbers a re-measurement needs.
- The warm-up gate never once reported `quiet` while blind. `warming_up`,
  `no_reference` and `quiet` are three distinct answers on the wire, and the
  reply carries `why_no_class` rather than a silent absence.
- `camera_capture` returns a real greyscale JPEG again (`FF D8` … `FF D9`),
  encoded on the node by `jpge` from the Y8 frame. Quality is honoured: 1, 5 and
  10 gave 2,894 / 5,982 / 22,540 bytes from the same 76,800-byte frame.

## The floor, measured on an empty bench — and it is the opposite of the prediction

`tests/fixtures/vision-floor-2026-09-17/` — 200 frames at 1 s from
`obc-esp32-s3-003`, with `first.jpg` and `last.jpg` in the same directory
showing an empty workshop corner at both ends of the run. Captured by
`scripts/vision/bench_floor.py`.

```
                    on-node Y8        host fixture (JPEG round trip)
frac   mean            0.0072                              0.041
       p95             0.0087                              0.068
       max             0.0095                              0.083
edge   mean             1.178                               2.49
       p95              1.26                                3.08
       max              1.28                                3.40
brightness        91.5 – 94.7 (3.2)                 108.7 – 111.0 (2.3)
judged            198 / 200
```

**The on-node floor is roughly nine times lower on `frac` and nearly three times
lower on `edge`.** The ADR predicted the opposite, in as many words: raw Y8
carries sensor noise that JPEG quantisation smooths away, so more pixels should
cross a 12-grey-level threshold and the floor should *rise*. It falls.

The likely mechanism, stated as a hypothesis because this run cannot prove it:
**the JPEG round trip was adding difference, not removing it.** Each frame is
quantised independently, so two nearly-identical sensor frames decode to two
visibly different images — block boundaries and DCT coefficients land
differently. The host fixture was not measuring the room's noise floor so much
as the encoder's. Differencing raw Y8 skips that entirely.

**What this run does not control for.** It is a different scene, a different
day, different light and a 1.0 s interval against the fixture's 0.75 s. Scene
and pipeline are confounded, and the honest claim is "the floor on this bench
with this pipeline is 0.0095 `frac` / 1.28 `edge`", not "JPEG was responsible
for the difference". The clean experiment is both pipelines on one scene, and it
is not expensive: capture JPEG and greyscale runs of the same still room.

### Consequence for the thresholds: safe, and far too loose

Zero of 199 scored frames would be called `motion` or `nudge` by the host
thresholds. They are not dangerous here. They are **enormously** slack —
`FRAC_HI` is 0.35 against a measured maximum of 0.0095, a factor of 37.

That is not licence to lower them. A floor sets a lower bound on where a
threshold may go; it says nothing about where the *events* sit, and a threshold
tuned to one class is how a detector acquires false positives on the other
three. `person`, `light_change` and `camera_nudge` have to be measured on-node
too, and until they are `thresholds_provisional` stays true on every reply.

### The warm-up gate holds up

198 of 200 frames judged, brightness holding 91.5–94.7 across three and a half
minutes. `SETTLE_DELTA = 3.0` was taken from the host fixture and is comfortable
here. Frame 0 is `no_reference`, frame 1 is `warming_up`, and from frame 2 the
node is judging — which is the designed behaviour and the first time it has been
seen on a scene quiet enough to show it.
