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

## First run on a Sense, from `main` (2026-09-26)

The first camera build from `main` (the `obc-esp32-s3-camera` crate, via
`scripts/build_camera.ps1 -Board xiao-sense`) on obc-esp32-s3-005: XIAO ESP32S3
Sense, OV3660, MAC `AC:27:6E:A8:4D:E4`, XCLK 10 MHz.

`probe_sense_capture.py --count 10`: 10/10 `frame 76800 B 320x240 format=3`.

`probe_detect.py 10 1.0 COM11`, a still scene on the bench:

    #  state        class  frac  edge   mean
    0  no_reference -      -     -      51.86
    1  warming_up   -      1.0   4.59   124.83
    2  warming_up   -      0.0   0.67   124.99
    3-9 ready       quiet  0.0   0.63-0.66   124.68-124.79

The warm-up gate did its job on a harder start than the Lilygo's: brightness
went 51.9 -> 124.8 between the first two frames, and frame 1 scored `frac` 1.0.
Without the gate, that frame would have been reported as a major event. After
that, 7 of 7 judged frames were `quiet`.

The quiet level is lower than the Lilygo floor above (`frac` 0.0 against a
0.0095 max, `edge` 0.63-0.66 against 1.28), on a different sensor, scene and
light, from 7 frames rather than 200. Treat it as a data point: it adds no
bound the Lilygo floor did not already give, and it says nothing about where
the events sit. `thresholds_provisional` stays true.

## Lamp switches on the node: the gate holds, the rule would not (2026-09-26)

`tests/fixtures/vision-events-2026-09-26/005/`, recorded with
`scripts/vision/bench_events.py` on obc-esp32-s3-005 (XIAO Sense, OV3660, raw
Y8, XCLK 10 MHz): 40 quiet frames, then 150 s with a lamp switched about every
15 s, each switch marked by the operator pressing Enter. Pictures at both ends.

```
quiet   40 frames, 36 judged   frac max 0.0043   edge max 1.25   brightness 124.0-128.3

switch  frame  node          frac    edge   rule alone   brightness
  1       43   warming_up   0.7662   4.71   motion       124.1 -> 154.3 (-> 127.4 next frame)
  2       61   warming_up   0.4716   4.35   motion       127.3 -> 115.5
  3       76   warming_up   0.4797   4.37   motion       115.1 -> 127.1
  4       91   warming_up   0.4611   4.32   motion       126.8 -> 115.3
  5      106   warming_up   0.4789   4.31   motion       114.9 -> 127.0
  6      121   warming_up   0.5249   4.02   light        125.9 -> 109.8
  7      136   warming_up   0.4919   4.43   motion       115.7 -> 127.8
  8      151   warming_up   0.5451   4.23   motion       126.5 -> 110.2

lamp    115 frames: 0 detections; all 96 judged frames `quiet`;
        2-3 frames from each switch to `ready`;
        between switches frac max 0.0068, edge max 1.25
```

**What the node did is right.** Every switch frame was held by the warm-up
gate, no lamp frame was ever reported as a detection, and the node was judging
again two or three frames later.

**What the rule would have done on its own is not.** Seven of eight switches
score `motion`. `EDGE_LIGHT` is 4.20, set above the host fixture's
`light_change` maximum of 3.28; on this sensor and pipeline a lamp switch
scores `edge` 4.02-4.71. The margin the rule was supposed to have against a
lamp is gone here, and **on 005 the gate is the only thing between a lamp and
a false detection.**

The gate's own margin is comfortable. Auto-exposure re-levels within one frame,
so the switch shows up as a single step of about 12 grey levels (lamp on
~127, off ~115), and `SETTLE_DELTA` is 3. Where that protection would end is a
light change that does not step brightness by 3 in one frame: a dimmer, a lamp
warming up, daylight. Those would reach the rule on their own. None was
measured here.

Why `edge` is higher than the host fixture's is not established. Two
candidates, neither tested: raw Y8 carries structure that JPEG quantisation
smoothed away (the floor run above already showed the two pipelines differ),
and a lamp is directional, so switching it moves shadows, which is a real
structural change. Different sensor, room and lamp from the fixture, so this
is one observation.

**Not changed:** the thresholds, and `thresholds_provisional: true`. Deciding
what to do about `EDGE_LIGHT` wants the second Sense's numbers and a person
measured on-node, neither of which exists yet. Moving a threshold to fit the
lamp alone is how a detector acquires false negatives on people.

Method note: the first version of the labeller looked one frame either side of
each Enter press. Switch 1 was pressed about 1.5 frames late, so it took the
auto-exposure correction frame as the switch and filed the flip itself, the
worst `edge` of the run, as a steady frame. The window is now two frames.

## The second Sense: same verdict from the gate, a different sensor under it (2026-09-26)

`tests/fixtures/vision-events-2026-09-26/002/`: the same session on
obc-esp32-s3-002, the other XIAO Sense, which has an **OV2640** where 005 has
an OV3660. Same build, same lamp. TV off and fan stopped: `last.jpg` shows a
dark screen and sharp fan blades. All 9 presses matched a switch (offsets 0 to
+2).

```
                        005 (OV3660)            002 (OV2640)
lamp switches           8, gate held 8          9, gate held 9
node detections         0                       0
frames to ready         2-3                     2-3
switch-frame edge       4.02-4.71               12.32-14.14
rule alone calls them   motion 7, light 1       nudge 9
quiet frac              <= 0.0043               0.105-0.113 (every frame)
quiet edge              <= 1.25                 7.33-7.66   (every frame)
between switches, lamp on  (~125)   frac <= 0.007    frac ~0.07, edge ~6.3
between switches, lamp off (~96-115) frac <= 0.007   frac ~0.13, edge ~8.3
```

**The node's behaviour is the same on both:** 17 real lamp switches, 17 held
by the warm-up gate, 0 detections, judging again within three frames.

**The rule alone holds a lamp on neither.** On 005 a lamp scores `motion`; on
002 it scores `nudge`, which on a judged frame would also drop the reference.
The host fixture put a lamp at `edge` <= 3.28, under `EDGE_LIGHT` 4.20. Here it
is 4.0-4.7 on one sensor and 12-14 on the other.

**002's floor is sensor noise, and it moves with the light.** Its quiet frames
change by the same amount every frame -- `frac` 0.105-0.113 -- which is what
independent per-pixel noise does (a fixed fraction of pixels crosses the
12-level threshold each frame); brightness crept 89.5 -> 97.1 over the phase
without frame-to-frame jumps, so it is not flicker banding. It tracks scene
brightness: ~0.07 with the lamp on, ~0.13 with it off, the signature of gain
rising in the dark. At matched brightness 005 is at <= 0.007. Read as Gaussian
noise, that is a per-pixel sigma of roughly 5 grey levels on 002 against 3 on
005 -- an estimate from `frac` alone, not a measurement of the sensor.

**Consequence for the thresholds, recorded, not acted on:**

* `EDGE_*` are absolute numbers, and the two Senses differ by ~6 in quiet
  `edge` alone. 002 sits at 7.5 doing nothing, 83% of `EDGE_NUDGE`. One
  threshold set cannot describe both; per-sensor thresholds, or thresholds
  relative to each node's own measured floor, is a design decision for later.
* `FRAC_HI` 0.35 still clears 002's floor, but by 2.3x in a dim scene, and the
  floor rose as the room darkened. A darker room than this one is the next
  thing that could put the rule into a `motion` call with nothing moving.
* The gate is doing the work on both sensors, and it relies on a lamp stepping
  brightness by more than 3 grey levels in a frame (here 12-37). A light change
  that does not -- a dimmer, daylight -- would reach the rule alone, which the
  numbers above say is not safe. Not measured.
* Still unmeasured on any node: a person, a camera nudge.
  `thresholds_provisional` stays true.

**The first 002 session** (`002-tv-fan/`) had a playing TV and a running fan in
view: quiet floor `frac` 0.117 and `edge` 7.8 on every frame, 7 presses against
9 flips (two unmarked, which the labeller now reports). With both moving in
frame, the node still judged every frame `quiet` and reported no detection --
small moving areas are not a detection, which is what the rule is for. It is
kept as that, not as a floor.

**Side observation, not investigated:** the first picture after the port-open
reset is dark (`first.jpg` brightness ~37 against ~95 for the frames after it),
as was `grab_picture.py`'s first JPEG and `probe_detect`'s first frame. The
driver appears to hand back a frame captured before auto-exposure settled.

## A person walking through: not seen (2026-09-26)

`tests/fixtures/vision-events-2026-09-26/person-005/`, `bench_events.py --mode
person` on obc-esp32-s3-005: 40 quiet frames, then seven cued walks across the
middle of the view, about 3 ft from the camera, which looks steeply down at the
floor. Pictures at both ends show the room empty; the monitor in view is
static. Scored with an 11 s window per walk (the first default, 8 s, was
short: the walker appeared ~3 s after each cue and was in view until ~+10 s).

```
quiet    38 judged, frac max 0.0018, edge max 1.27, 0 detections

walk  frames  judged  detected  frac max  edge max   rule alone on every frame
  1      8       1       no       0.82      4.68     motion 1, light 2, quiet 5
  2      8       2       no       0.15      2.99     quiet 8
  3      9       3       no       0.43      2.58     light 1, quiet 8
  4      8       4       no       0.23      3.08     quiet 8
  5      8       8       no       0.03      1.19     quiet 8   (walker at the edge of view)
  6      9       3       no       0.21      3.68     quiet 9
  7      9       4       no       0.23      3.39     quiet 9

between  64 frames, 0 detections
```

**0 of 7 walks detected.** Two mechanisms, each sufficient on its own:

1. **The warm-up gate withholds the frames the person is in.** A body crossing
   the view moves mean brightness by 5-15 grey levels frame to frame (114 ->
   142 in walk 1) as auto-exposure reacts to it. `SETTLE_DELTA` is 3, so the
   node answers `warming_up` for almost every frame with the walker in it:
   across the seven walks, two frames containing the walker were judged, both
   with the walker at the edge of view. The gate cannot tell "exposure is
   hunting" from "something large came in", and it was built to be blind
   through the first. The lamp sessions above are this same mechanism doing
   what it was built for.
2. **Where frames were scored, frac/edge put the person where a lamp is.** The
   walker scores `edge` 1.8-4.7; a lamp switch on the same node scored
   4.0-4.7. `frac` stays 0.07-0.24 for most walker frames -- under `FRAC_HI`
   0.35, set from a host fixture where the person leaned in close -- and where
   it is higher (walk 1: 0.59-0.82) `edge` is still at most 4.7. On this
   sensor and view, `edge` does not separate a person from a lamp.

**What this means.** On 005, as built, the detector is safe against lamps
because it is blind through any brightness step, and a person produces
brightness steps. Its two defences against the lamp (gate and `EDGE_LIGHT`)
are exactly what hide a walker. No single threshold change fixes both: loosen
the gate and lamps become detections through a rule that cannot tell them
apart; lower `FRAC_HI` or `EDGE_LIGHT` and the lamp crosses them first. What
would have to change is what is measured -- where in the frame the change is
(a person is a localised change, a lamp is global), or brightness judged
against more than one frame -- not the numbers. That is a design question,
not a tuning one, and nothing here changes the firmware.

`thresholds_provisional` stays true, and on this evidence the reply should not
be read as a person detector on 005 at all.

### The second Sense: not seen either, and called a nudge when it is judged

`tests/fixtures/vision-events-2026-09-26/person-002-1ft/`: the same session on
obc-esp32-s3-002 (OV2640), walking about **1 ft** from the camera -- closer
than 005's run, so the walker fills more of the frame. Frames take 1.3 s here,
so the walker was in view for two or three frames per walk, 3-7 s after each
cue. Room empty at the end; TV off.

```
quiet    38 judged, frac max 0.094, edge max 6.91 (002's noise floor, as in the lamp run)

walker frames, all seven walks:  frac 0.30-0.54   edge 8.8-13.5
judged walker frames:            3 (all in walk 6): quiet, quiet, nudge
rule alone on walker frames:     nudge 9, quiet the rest
walks detected:                  0 of 7;  between walks: 0 detections
```

**0 of 7 again, and the rule never says `motion` for this person either.**
The walker's `edge` (8.8-13.5) is above `EDGE_NUDGE` 9.0 or next to it, so
wherever `frac` crosses `FRAC_HI` the verdict is `nudge` -- the lamp's verdict
on this board, and one that drops the reference. Where `frac` stays just under
0.35 the verdict is `quiet`. The gate withheld most walker frames, as on 005.

### 002 again at 3 ft: the gate lets the walker through, and the rule still misses

`person-002-3ft/`: the same session a few minutes earlier, walking ~3 ft away
as on 005. Room empty at the end, darker than the 1 ft run.

```
quiet    38 judged, frac max 0.102, edge max 6.29

walker frames (frac > 0.15), all seven walks:   25, of which 18 judged
walker frac 0.15-0.27, edge 7.7-11.5            every judged one: quiet
walks detected: 0 of 7;  between walks: 0 detections
```

Here the gate is **not** what hides the walker: from 3 ft, in this lighting,
the body moved brightness too little to trip `SETTLE_DELTA` on most frames,
and 18 of 25 walker frames were judged. The rule called every one `quiet`,
because the walker changes 15-27% of the pixels and `FRAC_HI` is 0.35 -- even
though `edge` sat at 7.7-11.5 against a quiet floor of 6.3, a difference the
rule never gets to use once `frac` has said `quiet`. So the two failures are
separate: close up the gate hides the person; further away the whole-frame
`frac` is too small to register them. Either alone is enough for 0 of 7.

### Both Senses, one conclusion

| | 005 (OV3660), ~3 ft | 002 (OV2640), ~3 ft | 002 (OV2640), ~1 ft |
|---|---|---|---|
| walks detected | 0 / 7 | 0 / 7 | 0 / 7 |
| walker `frac` | mostly 0.07-0.24 | 0.15-0.27 | 0.30-0.54 |
| walker `edge` | 1.8-4.7 | 7.7-11.5 | 8.8-13.5 |
| lamp switch `edge`, same board | 4.0-4.7 | 12.3-14.1 | 12.3-14.1 |
| what the rule makes of the walker | `quiet` / `light` (a lamp) | `quiet` | `quiet` / `nudge` (a lamp, or a bump) |
| walker frames the gate let through | 2 | 18 of 25 | 3 |

On each board the person lands either where that board's lamp lands or under
`FRAC_HI`, and wherever the walker moves the brightness the gate hides them. The two whole-frame numbers the detector keeps
-- how many pixels changed, and how much structure changed -- do not carry the
difference between a person and a lamp, on either sensor, at either distance.
What does differ is **where**: a lamp changes the whole frame at once, a
walker changes the part of it they are in. That is the input to the next
design, which is a proposal to write, not a change made here.

**Next:** `docs/VISION-DETECTOR-PROPOSAL-2026-09.md` scores five candidate
redesigns against the host fixture and two simulations of these failures, and
recommends per-cell correlation in place of the warm-up gate -- after frames
recorded on the Senses have confirmed it.
