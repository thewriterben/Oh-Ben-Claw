#!/usr/bin/env python3
"""Freeze a few real frame pairs as raw grey, with the reference scores.

    python scripts/vision/make_grey_pairs.py

Writes `tests/fixtures/vision-grey-pairs-2026-09-17/`:
    <label>_prev.gray, <label>_cur.gray   -- 8-bit grey, WIDTHxHEIGHT, row-major
    expected.json                          -- what classify.py scores them

# Why this exists

`firmware/obc-esp32-s3/src/detector_math.rs` is a second implementation of
`classify.py`. Two copies of one rule is the exact shape that let `camera.rs`
contradict its own `Cargo.toml` for weeks, unnoticed because nothing compared
them. `tests/firmware_detector_math.rs` compares them, and it needs pixels the
Rust side can read without a JPEG decoder.

# Why crops, and what that costs

These are 160x120 centre crops, not the full 320x240 frames. Eight full frames
would be 614 KB of incompressible-ish raw in the tree to answer a question that
does not need them: the test asks **do the two implementations agree on the same
pixels**, not **are the thresholds right**. The thresholds are the 148-frame
fixture's job.

So the numbers in `expected.json` are NOT the headline numbers in
`docs/VISION-DETECTOR-2026-09.md` -- different pixel count, different borders,
different crop of the scene. They are whatever the reference says about these
exact buffers, which is all the comparison requires. The labels are carried
through only to show the pairs span the interesting cases.
"""

import json
import os
import sys

import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
SRC = os.path.join(REPO, "tests", "fixtures", "vision-bench-2026-09-16")
OUT = os.path.join(REPO, "tests", "fixtures", "vision-grey-pairs-2026-09-17")

sys.path.insert(0, HERE)
from classify import PIXEL_EPS, classify  # noqa: E402  (the reference itself)

CROP_W, CROP_H = 160, 120

# One pair per labelled set. The indices are the deltas that the 2026-09-16
# analysis found interesting: the peak frame of each set, and a quiet pair from
# the empty bench.
PAIRS = [
    ("baseline_still", 10, 11),
    ("light_change", 18, 19),
    ("person", 20, 21),
    ("camera_nudge", 3, 4),
]


def load_crop(label, index):
    path = os.path.join(SRC, label, f"{index:03d}.jpg")
    with Image.open(path) as im:
        a = np.asarray(im.convert("L"), dtype=np.uint8)
    h, w = a.shape
    y0 = (h - CROP_H) // 2
    x0 = (w - CROP_W) // 2
    return np.ascontiguousarray(a[y0:y0 + CROP_H, x0:x0 + CROP_W])


def main():
    if not os.path.isdir(SRC):
        print(f"source fixture not found: {SRC}")
        return 2
    os.makedirs(OUT, exist_ok=True)

    rows = []
    for label, i_prev, i_cur in PAIRS:
        prev = load_crop(label, i_prev)
        cur = load_crop(label, i_cur)
        with open(os.path.join(OUT, f"{label}_prev.gray"), "wb") as fh:
            fh.write(prev.tobytes())
        with open(os.path.join(OUT, f"{label}_cur.gray"), "wb") as fh:
            fh.write(cur.tobytes())

        cls, frac, edge = classify(
            prev.astype(np.float32), cur.astype(np.float32)
        )
        rows.append({
            "label": label,
            "source": f"{label}/{i_prev:03d}.jpg -> {label}/{i_cur:03d}.jpg",
            "width": CROP_W,
            "height": CROP_H,
            "pixel_eps": PIXEL_EPS,
            "frac": frac,
            "edge": edge,
            "mean": float(cur.astype(np.float32).mean()),
            "class": cls,
        })
        print(f"  {label:16s} frac={frac:.6f} edge={edge:.6f} -> {cls}")

    meta = {
        "note": (
            "160x120 centre crops of tests/fixtures/vision-bench-2026-09-16. "
            "Scores are the reference implementation's (scripts/vision/classify.py) "
            "on these exact buffers -- NOT the full-frame numbers in "
            "docs/VISION-DETECTOR-2026-09.md. Regenerate with "
            "scripts/vision/make_grey_pairs.py."
        ),
        "pairs": rows,
    }
    with open(os.path.join(OUT, "expected.json"), "w", encoding="utf-8") as fh:
        json.dump(meta, fh, indent=2)
        fh.write("\n")
    print(f"\nwrote {len(rows)} pairs into {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
