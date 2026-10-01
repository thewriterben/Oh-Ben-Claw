# Detector redesign: options, scored (2026-09-26)

**Status: decided 2026-09-26 -- design E, per-cell correlation; a detection
means "take a picture now". Next is recording real frames on the Senses (below).
Nothing here changes the firmware yet.**

`docs/VISION-DETECTOR-2026-09.md` ends with the bench result that forces this:
across 21 walks on two XIAO Senses, the on-node detector detected **no one**.
Two failures, either sufficient: close to the camera, the warm-up gate withholds
the frames a person is in (a body moves the brightness, auto-exposure reacts);
further away, a person changes too little of the frame for whole-frame `frac`
to register, and where they change more, `edge` puts them where a lamp is.

Both say the same thing. A lamp and auto-exposure change the whole frame in a
way a brightness model can absorb. A person changes *part* of the frame in a way
no brightness model can. The current detector keeps two whole-frame numbers and
so cannot see that difference.

## The options

| | design | per frame | replaces the gate? |
|---|---|---|---|
| **A** | current rule: whole-frame `frac` + `edge` (scored here without the gate) | 2 numbers | no |
| **A′** | A with only `FRAC_HI` lowered -- "just tune it", tested rather than argued | 2 numbers | no |
| **B** | gain-compensated `frac`: fit `cur ≈ g·prev + o` over the frame (refit without outliers), count pixels off the fit | 2 passes | yes, in principle |
| **C** | grid, raw difference: 8×6 cells (40×40 px), mean \|diff\| per cell | 1 pass | no |
| **D** | B + C: per-cell share of pixels off the whole-frame brightness fit | 2 passes | yes, in principle |
| **E** | per-cell correlation (NCC): `1 − corr(prev, cur)` per 40×40 cell, which ignores a gain and offset *inside each cell* | 1 pass, 5 sums per cell | yes |

For B–E, a verdict is `detect` if any cell (or the frame, for B) is over its
threshold, and `global` if more than 60% of cells are -- a nudge or a scene-wide
change, reported and not called a detection, as `nudge` is today. All are cheap
at 320×240 Y8 and one frame a second; E needs, per cell, the sums of `p`, `c`,
`p²`, `c²` and `p·c`.

## How they were scored

`scripts/vision/compare_detectors.py` -- rerun it to reproduce every number here.

* **Real pixels:** the 2026-09-16 host fixture, 148 frames, LILYGO (OV5640).
  **A person is in frame in every set** (checked by eye): the "quiet" set is
  them keeping still, "person" is them moving, and "light_change" is a lamp
  switching while they stand in its light. None of it is a person *arriving*,
  which is what the Senses missed.
* **Simulated, because no raw frames of today's failures exist:**
  `ae_step` -- the quiet pairs with the current frame scaled by 0.85 and 1.15,
  the size of step walkers caused on 005, which the answer must call quiet;
  `small_person` -- each person pair shrunk to a quarter of its area and pasted
  into an empty-ish quiet frame, standing in for twice the distance.
* **Calibration, the same for every design but A:** threshold = 1.5 × the
  largest value the design scores on the quiet set. Nothing is tuned on the
  lamp, AE or person frames. A's thresholds were hand-set on this fixture,
  lamp and person included, so A is flattered, not handicapped.

Two calibrations are reported, because the choice mattered as much as the
design.

### One threshold per design (the quiet maximum anywhere in the frame)

```
  test                   pairs             A            A'             B             C             D             E
                               detect/global detect/global detect/global detect/global detect/global detect/global
  baseline_still            28        0/0           0/0           0/0           0/0           0/0           0/0      (must be 0 detect)
  ae_step (sim)             56        2/0           2/0           8/0          28/0          28/0           0/0      (must be 0 detect)
  lamp on a lit person      38        0/0           0/0          18/0          20/0          21/0          17/0      (see note)
  camera_nudge              38        2/8           3/8          12/0           7/6          10/1          12/0      (few detect; global on the nudge)
  person                    38       20/0          23/0          26/0          28/4          27/3          28/0      (detect: more is better)
  small_person (sim)        38        0/0           5/0          20/0          29/0          22/0          17/0      (detect: more is better)
```

### One threshold per cell (each cell's own quiet maximum)

```
  test                   pairs             A            A'             B             C             D             E
                               detect/global detect/global detect/global detect/global detect/global detect/global
  baseline_still            28        0/0           0/0           0/0           0/0           0/0           0/0      (must be 0 detect)
  ae_step (sim)             56        2/0           2/0           8/0           0/56         28/0          28/0      (must be 0 detect)
  lamp on a lit person      38        0/0           0/0          18/0          20/18         26/12         35/3      (see note)
  camera_nudge              38        2/8           3/8          12/0          26/12         27/11         27/11     (few detect; global on the nudge)
  person                    38       20/0          23/0          26/0           7/28         16/20         16/19     (detect: more is better)
  small_person (sim)        38        0/0           5/0          20/0          32/0          32/0          32/0      (detect: more is better)
```

## What the numbers say, and what they cannot

1. **Tuning the current rule does not fix it.** A′ (FRAC_HI lowered from 0.35
   to 0.12) finds 5 of 38 simulated distant people; A finds 0. Whole-frame
   numbers do not carry the difference, as the bench said.
2. **Every region design finds the distant person** the whole-frame designs
   miss: 17-32 of 38 against 0-5.
3. **E is the only design with no false detections on the quiet set and the AE
   step** under the first calibration, while finding more of the close person
   than A (28 vs 20) and the distant one (17 vs 0). That is the property the
   gate was there to provide, obtained without the gate -- so it does not go
   blind while a person walks through.
4. **Calibration is the open problem.** Per-cell thresholds from 28 quiet pairs
   are too tight: a cell that never changed gets a near-zero bar, and the AE
   step trips it (E: 0 → 28 false). A single threshold is robust here but was
   set by the noisiest quiet cell -- E's came out at 1.01, meaning a cell must
   decorrelate completely -- because the quiet set has a person in it. An empty
   room would give a much lower one. Either way, a node needs a calibration
   step on its own empty scene, long enough to see its noise.
5. **The lamp question is not answered.** Every region design flags the lamp
   lighting a person's face (17-35 of 38). Whether that is wrong depends on
   what a detection is for, and this fixture has no lamp without a person.
6. **One sensor, host JPEG, one room.** The Senses (OV3660, OV2640) at 1 fps,
   raw Y8, looking down or across, are what has to work.

## Recommendation

**E, per-cell correlation, replacing the warm-up gate** -- with two
preconditions before any firmware changes:

1. **Record the real thing.** `bench_events.py` gains a mode that saves a
   picture every frame (`camera_capture`, ~11 KB at quality 8), and we record
   on 005 and 002: an empty room for a few minutes, lamp switches with nobody
   in view, and walks at 1 ft and 3 ft. Then `compare_detectors.py` runs on
   those frames instead of the host fixture and simulations. If E does not hold
   up there, we find out before the firmware does.
2. **Decide what a detection is for** (a presence alert over LoRa, a trigger
   for a picture, an activity count). It sets the answer to the lamp question
   and how many false detections are tolerable.

Then the firmware change is small and testable the way the current one is: E
in `detector_math.rs`, the same statistic in Python, the
`tests/firmware_detector_math.rs` agreement test extended, and an empty-room
calibration command that stores per-node thresholds.

### The alternatives, and why not

* **A′ (tune the thresholds):** measured insufficient (point 1).
* **B (gain-compensated whole frame):** fixes auto-exposure in principle and is
  the smallest code change, but stays whole-frame: 20 of 38 distant people, and
  8 false detections on the AE step. A step, not a fix.
* **C (grid, raw difference):** best at the distant person (29-32 of 38) but
  cannot tell a brightness step from a change (28 false on AE with one
  threshold), so it keeps needing the gate -- the thing that hid people close up.
* **D (gain-compensated grid):** as C on AE (28 false), and the whole-frame fit
  is dragged by a large nearby person.

## Decision (2026-09-26)

**Design E, per-cell correlation, replacing the warm-up gate. A detection is a
trigger to take a picture.** What that purpose settles:

* **A miss is the expensive error.** A false trigger costs one picture nobody
  needed; a missed person costs the only picture that mattered. Thresholds lean
  toward sensitivity, and the bench numbers to beat are walks detected.
* **Latency matters.** The picture has to be taken while the person is still in
  view, so the useful number is seconds from a person appearing to the first
  detection, and the first frame they are in should be enough.
* **A lamp lighting a person who is in view is a fine moment for a picture.**
  The false trigger that counts is a lamp, or the camera's own exposure, with
  nobody in view -- which the host fixture never contains and a recording on
  the Senses will.

## Recording the Senses

`bench_events.py --record` runs the usual lamp or person session but saves a
picture every frame (`camera_capture`) instead of asking the node's detector,
into `results/vision-events/<node>-<stamp>-rec-<mode>/frames/`.
`compare_detectors.py --session DIR` then labels those frames with the same
code that labels the node's own runs (Enter marks for lamp switches, cue
windows for walks), calibrates every design on that recording's own quiet
phase -- an empty room, as a node would on install -- and reports, per design:

* lamp, nobody in view: detections on switch, unmarked, resettling and steady
  frames, every one of them a false trigger;
* person: walks detected, seconds from `WALK NOW` to the first detection, and
  detections between walks.

Wanted: on 005 and 002, a lamp session with nobody in view, and person sessions
at ~1 ft and ~3 ft. Dry-run end to end against a fake node serving fixture
frames; no real recording exists yet.
