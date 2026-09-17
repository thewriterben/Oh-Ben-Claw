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
