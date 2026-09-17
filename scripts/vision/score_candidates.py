"""Five candidate scores, compared on the same labelled sets.

Added after `norm` was refuted on 2026-09-16: subtracting each frame's mean did
NOT suppress a lighting change (raw p50 9.17 -> norm p50 9.34). Mean-subtraction
cancels a uniform ADDITIVE shift; a lamp is a multiplicative gain landing
unevenly on the scene, so there was nothing for it to cancel.

  raw    mean |f - p|                               the naive baseline
  norm   mean-subtracted                            (kept to show it failing)
  gain   each frame divided by its own mean         matches the physics of a lamp
  edge   mean |grad(f) - grad(p)|                   structure, not brightness
  edgeg  gradient of the gain-normalised frames     both at once

`frac_*` variants report the fraction of pixels exceeding a threshold, which is
the localisation signal: a moving object lights up a patch, a lighting change or
a camera nudge lights up nearly everything.
"""

import os
import sys

import numpy as np
from PIL import Image

ROOT = r"C:\Users\Benji\obc-bench\dataset"


def load(p):
    with Image.open(p) as im:
        return np.asarray(im.convert("L"), dtype=np.float32)


def grad(a):
    gy = np.zeros_like(a)
    gx = np.zeros_like(a)
    gy[1:-1, :] = a[2:, :] - a[:-2, :]
    gx[:, 1:-1] = a[:, 2:] - a[:, :-2]
    return np.hypot(gx, gy)


def series(label, skip_first=True):
    d = os.path.join(ROOT, label)
    fs = sorted(f for f in os.listdir(d) if f.endswith(".jpg"))
    if skip_first:
        fs = fs[1:]          # the auto-exposure warm-up frame
    frames = [load(os.path.join(d, f)) for f in fs]
    out = {k: [] for k in ("raw", "norm", "gain", "edge", "edgeg",
                           "frac_raw", "frac_gain", "frac_edgeg")}
    for p, c in zip(frames, frames[1:]):
        out["raw"].append(np.abs(c - p).mean())
        out["norm"].append(np.abs((c - c.mean()) - (p - p.mean())).mean())

        cg = c / max(c.mean(), 1e-6) * 128.0
        pg = p / max(p.mean(), 1e-6) * 128.0
        dg = np.abs(cg - pg)
        out["gain"].append(dg.mean())

        de = np.abs(grad(c) - grad(p))
        out["edge"].append(de.mean())

        deg = np.abs(grad(cg) - grad(pg))
        out["edgeg"].append(deg.mean())

        out["frac_raw"].append((np.abs(c - p) > 12).mean())
        out["frac_gain"].append((dg > 12).mean())
        out["frac_edgeg"].append((deg > 12).mean())
    return {k: np.array(v) for k, v in out.items()}


labels = sys.argv[1:]
data = {l: series(l) for l in labels}
keys = ["raw", "norm", "gain", "edge", "edgeg", "frac_raw", "frac_gain", "frac_edgeg"]

for l in labels:
    print(f"\n  {l}")
    for k in keys:
        a = data[l][k]
        print(f"    {k:11} mean {a.mean():8.3f}  p50 {np.percentile(a,50):8.3f}"
              f"  p95 {np.percentile(a,95):8.3f}  max {a.max():8.3f}")

base = next((l for l in labels if l.startswith("baseline")), None)
if base and len(labels) > 1:
    print("\n  === discrimination: how far each set sits above the quiet floor ===")
    print("  (ratio of this set's p50 to baseline p95 -- a score that suppresses an")
    print("   effect lands near 1.0; one that fires on it lands high)\n")
    hdr = "  " + "set".ljust(16) + "".join(k.rjust(12) for k in keys)
    print(hdr)
    for l in labels:
        if l == base:
            continue
        row = "  " + l.ljust(16)
        for k in keys:
            b95 = np.percentile(data[base][k], 95)
            r = np.percentile(data[l][k], 50) / b95 if b95 > 1e-9 else float("inf")
            row += f"{r:12.1f}"
        print(row)
