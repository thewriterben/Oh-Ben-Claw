#!/usr/bin/env python3
"""Three-way frame classifier, and the regression check for its thresholds.

    python scripts/vision/classify.py              # report
    python scripts/vision/classify.py --check      # exit 1 if the numbers moved

Runs against `tests/fixtures/vision-bench-2026-09-16/`, 148 frames captured from
node `obc-esp32-s3-4b95f8` (LILYGO T-CameraPlus-S3 V1.1, OV5640) on 2026-09-16.

# Why this exists

The plan was a frame-difference score, and the obvious one does not work. Three
things in the same room produce a large whole-frame delta and only one of them
is a detection:

  * a lamp switching on  -- every pixel changes, nothing moved
  * the camera nudged    -- every pixel changes, the world is identical
  * a person walking in  -- the real event

A naive `mean|f - prev|` scores all three alike, and on this bench the first two
are the common case. A detector tuned without them reports a night of activity
that was one light switch -- the same alarm-fatigue failure the mesh escalations
already hit, arriving in the vision tier.

# What was measured, and what was refuted

Two illumination-normalising scores were proposed on physics and BOTH lost:

  norm   subtract each frame's mean      raw p50 7.65 -> norm p50 7.99. No help.
                                         Mean-subtraction cancels a uniform
                                         ADDITIVE shift; a lamp is a
                                         multiplicative gain landing unevenly,
                                         so there was nothing to cancel.
  gain   divide by each frame's mean     WORSE than doing nothing: 2.4x the
                                         quiet floor vs raw's 2.1x, and its
                                         fraction variant 3.6x. It amplified
                                         the thing it was meant to remove.

The winner was not proposed in advance: `frac` (how MANY pixels changed) for
presence, and `edge` (did STRUCTURE change) for what changed. Localisation and
structure, not brightness arithmetic.

# The floor

Baseline, 28 deltas of an empty bench, after discarding frame 000:

    raw   mean  2.62  p95  3.66  max  4.61
    edge  mean  2.49  p95  3.08  max  3.40
    frac  mean 0.041  p95 0.068  max 0.083

Frame 000 is discarded on purpose and it is a finding, not tidying: it read
brightness 62.9 against a steady 108.7-111.0, `raw` 80.6, 87% of pixels changed.
That is auto-exposure converging after the port-open reset. A detector armed at
boot calls it a major event every single boot, so the node must withhold
judgement until brightness settles and SAY it is warming up.

# Peaks, per labelled set

                     raw    frac    edge
    baseline         4.6   0.083     3.4
    light_change    14.7   0.309     3.3    <- edge at the floor: suppressed
    person          66.3   0.943     7.7
    camera_nudge   101.4   0.966    13.1

`frac` cannot separate person from nudge (0.943 vs 0.966). `edge` can, because a
lighting change leaves structure alone while a nudge moves every edge at once.

# Honest limits

148 frames, one bench, one afternoon, one lighting setup, one sensor. This is a
FIXTURE, not a validation. The thresholds below are measured, which beats tuned,
and they are still three numbers chosen to fit four recordings.

Known misclassifications, kept in the expected counts rather than smoothed away:

  * 3 `person` frames land in `light` (edge 4.02-4.19, just under the boundary).
    Under-reporting.
  * 2 `camera_nudge` frames land in `motion` at deltas 0 and 2 -- BEFORE the
    nudge at delta 3. Almost certainly a hand entering frame to reach the
    camera, in which case they are true motion and the SET LABEL is what is
    wrong. Not provable from these numbers, so it is recorded as an open
    question rather than excused.
"""

import os
import sys

import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.environ.get(
    "OBC_VISION_FIXTURE",
    os.path.join(HERE, "..", "..", "tests", "fixtures", "vision-bench-2026-09-16"),
)

# Thresholds, from the measured distributions above.
FRAC_HI = 0.35      # above light_change's max (0.309), well under person's p50 (0.652)
EDGE_LIGHT = 4.20   # above baseline max (3.40) and light_change max (3.28)
EDGE_NUDGE = 9.00   # between person max (7.66) and camera_nudge max (13.11)
PIXEL_EPS = 12.0    # grey levels

# What the fixture produced when the thresholds were set. A change here is a
# real change in behaviour and wants an explanation, not an update.
EXPECTED = {
    "baseline_still": {"quiet": 28, "light": 0, "motion": 0, "nudge": 0},
    "light_change": {"quiet": 38, "light": 0, "motion": 0, "nudge": 0},
    "person": {"quiet": 15, "light": 3, "motion": 20, "nudge": 0},
    "camera_nudge": {"quiet": 26, "light": 2, "motion": 2, "nudge": 8},
}
CLASSES = ["quiet", "light", "motion", "nudge"]


def load(path):
    with Image.open(path) as im:
        return np.asarray(im.convert("L"), dtype=np.float32)


def grad(a):
    gy = np.zeros_like(a)
    gx = np.zeros_like(a)
    gy[1:-1, :] = a[2:, :] - a[:-2, :]
    gx[:, 1:-1] = a[:, 2:] - a[:, :-2]
    return np.hypot(gx, gy)


def classify(prev, cur):
    """(class, frac, edge) for one frame pair. The whole rule is these 6 lines."""
    frac = float((np.abs(cur - prev) > PIXEL_EPS).mean())
    edge = float(np.abs(grad(cur) - grad(prev)).mean())
    if frac <= FRAC_HI:
        return "quiet", frac, edge
    if edge <= EDGE_LIGHT:
        return "light", frac, edge
    if edge >= EDGE_NUDGE:
        return "nudge", frac, edge
    return "motion", frac, edge


def counts_for(label):
    d = os.path.join(ROOT, label)
    files = sorted(f for f in os.listdir(d) if f.endswith(".jpg"))[1:]  # AE warm-up
    frames = [load(os.path.join(d, f)) for f in files]
    counts = dict.fromkeys(CLASSES, 0)
    for p, c in zip(frames, frames[1:]):
        counts[classify(p, c)[0]] += 1
    return counts


def main():
    check = "--check" in sys.argv
    if not os.path.isdir(ROOT):
        print(f"fixture not found: {ROOT}")
        return 2

    print(f"  FRAC_HI={FRAC_HI}  EDGE_LIGHT={EDGE_LIGHT}  EDGE_NUDGE={EDGE_NUDGE}\n")
    print("  " + "set".ljust(16) + "".join(c.rjust(9) for c in CLASSES))
    failures = []
    for label, expected in EXPECTED.items():
        got = counts_for(label)
        row = "  " + label.ljust(16) + "".join(str(got[c]).rjust(9) for c in CLASSES)
        if got != expected:
            row += "   != expected " + str(expected)
            failures.append((label, expected, got))
        print(row)

    # The property that matters most, stated as its own assertion: a quiet room
    # and a lighting change must produce NO detections. Everything else is
    # tuning; this is the reason the detector exists.
    quiet_sets = ("baseline_still", "light_change")
    for label in quiet_sets:
        got = counts_for(label)
        loud = got["motion"] + got["nudge"]
        if loud:
            failures.append((label, "no detections", f"{loud} detections"))
            print(f"\n  FALSE POSITIVE: {label} produced {loud} detection(s)")

    if check and failures:
        print(f"\n  FAILED: {len(failures)} regression(s)")
        for label, exp, got in failures:
            print(f"    {label}: expected {exp}, got {got}")
        return 1
    if check:
        print("\n  ok: classifier matches the recorded fixture")
    return 0


if __name__ == "__main__":
    sys.exit(main())
