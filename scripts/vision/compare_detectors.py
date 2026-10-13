#!/usr/bin/env python3
"""Score candidate detector designs against the frames we have, and against the
two failures the 2026-09-26 bench found.

    python scripts/vision/compare_detectors.py
    python scripts/vision/compare_detectors.py --session DIR [DIR ...]   # recordings

# Why this exists

On 2026-09-26 the on-node detector missed 21 of 21 walks on two XIAO Senses
(docs/VISION-DETECTOR-2026-09.md). Two failures, each sufficient:

  * **the gate** -- a body in view moves mean brightness as auto-exposure
    reacts, and the warm-up gate withholds judgement on any 3-level step;
  * **whole-frame scores** -- a person far from a wide lens changes 15-27% of
    the pixels, under FRAC_HI, and where they change more, `edge` puts them
    where a lamp is.

Both point at the same thing: a lamp and auto-exposure change the whole frame
in a way a brightness model can absorb; a person changes part of it in a way
none can. This script scores designs that use that, side by side with the
current rule, before any of them goes near the firmware.

# What it is scored on, and what that cannot tell you

**Real pixels:** `tests/fixtures/vision-bench-2026-09-16/`, 148 frames from the
LILYGO (OV5640), host-decoded JPEG, one room, one afternoon. Labels are per SET,
not per frame. **A person is in frame, close, in every set** (checked by eye,
2026-09-26): `baseline_still` is that person keeping still, `person` is them
moving, and `light_change` is a lamp switching while they stand in its light
-- the change there is mostly their face going from dark to lit. So:

  * the quiet calibration includes a still person's small movements;
  * the lamp row is reported, not asserted: a design that flags a face
    lighting up is not obviously wrong, and this fixture cannot say;
  * none of it is a person *arriving*, which is what the Senses missed.

**Simulated, because no raw frames of today's failures exist:**

  * `ae_step` -- every consecutive pair of the quiet and lamp sets with the
    current frame scaled by a gain of 0.85 and 1.15 (a 15% step, what the
    walkers caused on 005). The gate existed to make this safe; a design that
    needs no gate must score it quiet.
  * `small_person` -- each person-set pair shrunk to half size (a quarter of
    the area, standing in for twice the distance) and pasted into the middle of
    a quiet baseline pair, so everything outside it is the empty room.

Both are stand-ins. The real test is frames recorded on the Senses, which this
cannot replace -- see the proposal doc for how to collect them.
"""

from __future__ import annotations

import os
import sys

import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.join(HERE, "..", "..", "tests", "fixtures", "vision-bench-2026-09-16")
SETS = ("baseline_still", "light_change", "person", "camera_nudge")

sys.path.insert(0, HERE)
from classify import EDGE_LIGHT, EDGE_NUDGE, FRAC_HI, PIXEL_EPS, grad  # noqa: E402

GRID_X, GRID_Y = 8, 6          # 40x40-pixel cells at 320x240
# A cell must have texture for correlation to mean anything: a flat patch of
# wall correlates with nothing, so its NCC is noise. Grey-level std below this
# (either frame) and the cell abstains.
MIN_CELL_STD = 4.0
# "Global" -- more than this share of the cells changed at once. A person, even
# close, did not do that in the fixture's 1-ft sense; a nudge and a lamp do.
GLOBAL_SHARE = 0.6


def load_set(label):
    d = os.path.join(ROOT, label)
    files = sorted(f for f in os.listdir(d) if f.endswith(".jpg"))[1:]  # AE warm-up, as classify.py
    return [np.asarray(Image.open(os.path.join(d, f)).convert("L"), dtype=np.float32) for f in files]


def cells(a):
    h, w = a.shape
    ch, cw = h // GRID_Y, w // GRID_X
    return a[: ch * GRID_Y, : cw * GRID_X].reshape(GRID_Y, ch, GRID_X, cw).swapaxes(1, 2)


# ── the candidates ───────────────────────────────────────────────────────────
#
# Every candidate is a *statistic*, per cell or for the whole frame, and one
# shared decision. Thresholds are NOT chosen by hand: each is calibrated as
# MARGIN x the largest value the statistic takes on the quiet set
# (baseline_still), and nothing is tuned on the lamp, AE or person frames. That
# is the only fair way to compare designs whose numbers mean different things,
# and it is also how a node could calibrate itself on an empty room.
#
# The current rule (A) is the exception and is marked so: its thresholds were
# set on this very fixture, lamp and person included.

MARGIN = 1.5


def s_frac(prev, cur):
    return np.array([[float((np.abs(cur - prev) > PIXEL_EPS).mean())]])


def gain_fit(prev, cur):
    """cur ~= g*prev + o, refit once without the worst-fitting pixels.

    The refit is what keeps a person from dragging the fit toward themselves:
    the first pass is ordinary least squares, the second drops pixels whose
    residual exceeds 3x its median absolute value.
    """
    x, y = prev.ravel(), cur.ravel()
    g, o = np.polyfit(x, y, 1)
    r = np.abs(y - (g * x + o))
    keep = r <= 3 * max(np.median(r), 1.0)
    g, o = np.polyfit(x[keep], y[keep], 1)
    return cur - (g * prev + o)


def s_gain(prev, cur):
    """B: share of the frame off a single brightness model."""
    return np.array([[float((np.abs(gain_fit(prev, cur)) > PIXEL_EPS).mean())]])


def s_grid(prev, cur):
    """C: per-cell mean |difference|, raw."""
    return cells(np.abs(cur - prev)).mean(axis=(2, 3))


def s_gain_grid(prev, cur):
    """D: per-cell share of pixels off the whole-frame brightness model."""
    return cells(np.abs(gain_fit(prev, cur)) > PIXEL_EPS).mean(axis=(2, 3))


def s_ncc(prev, cur):
    """E: per-cell 1 - correlation. Invariant to a gain and offset inside each
    cell, so a lamp or an exposure step (even an uneven one) leaves it near 0,
    and a change of content raises it. Untextured cells abstain (0) unless they
    went textured<->flat, which is a change of content and scores 1."""
    p, c = cells(prev), cells(cur)
    pz = p - p.mean(axis=(2, 3), keepdims=True)
    cz = c - c.mean(axis=(2, 3), keepdims=True)
    ps, cs = pz.std(axis=(2, 3)), cz.std(axis=(2, 3))
    tp, tc = ps >= MIN_CELL_STD, cs >= MIN_CELL_STD
    ncc = (pz * cz).mean(axis=(2, 3)) / np.maximum(ps * cs, 1e-6)
    out = np.where(tp & tc, 1.0 - ncc, 0.0)
    return np.where(tp != tc, 1.0, out)


def a_current(prev, cur):
    """A: the rule on the node today, without the gate. Hand-tuned on this fixture."""
    frac = float((np.abs(cur - prev) > PIXEL_EPS).mean())
    edge = float(np.abs(grad(cur) - grad(prev)).mean())
    if frac <= FRAC_HI or edge <= EDGE_LIGHT:
        return "quiet"
    return "global" if edge >= EDGE_NUDGE else "detect"


class Calibrated:
    """A statistic plus a threshold learned from quiet pairs only.

    Grid statistics get one threshold PER CELL (each cell's own quiet maximum),
    which is what a node calibrating on its empty room would learn: a cell over
    a flickering screen or a noisy dark corner gets a higher bar than a cell
    over a still wall, instead of the noisiest cell setting the bar for all.
    """

    def __init__(self, stat, quiet_pairs, grid, per_cell=True):
        self.stat, self.grid = stat, grid
        stack = np.stack([stat(p, c) for p, c in quiet_pairs])
        # per_cell=False: one threshold, the quiet set's maximum anywhere.
        self.cell_thr = MARGIN * (stack.max(axis=0) if per_cell else stack.max())
        self.thr = float(np.median(self.cell_thr))  # for the report

    def __call__(self, prev, cur):
        changed = self.stat(prev, cur) > self.cell_thr
        if self.grid and changed.mean() > GLOBAL_SHARE:
            return "global"
        return "detect" if changed.any() else "quiet"


def a_prime(quiet_pairs):
    """A': the current rule with only FRAC_HI recalibrated on the quiet set --
    "just lower the threshold", tested rather than argued."""
    frac_hi = MARGIN * max(float(s_frac(p, c).max()) for p, c in quiet_pairs)

    def fn(prev, cur):
        frac = float((np.abs(cur - prev) > PIXEL_EPS).mean())
        edge = float(np.abs(grad(cur) - grad(prev)).mean())
        if frac <= frac_hi or edge <= EDGE_LIGHT:
            return "quiet"
        return "global" if edge >= EDGE_NUDGE else "detect"
    fn.thr = frac_hi
    return fn


def build(quiet_pairs, per_cell):
    cands = [("A  current rule*", a_current),
             ("A' lower FRAC_HI", a_prime(quiet_pairs)),
             ("B  gain-comp frac", Calibrated(s_gain, quiet_pairs, grid=False)),
             ("C  grid, raw", Calibrated(s_grid, quiet_pairs, grid=True, per_cell=per_cell)),
             ("D  gain-comp grid", Calibrated(s_gain_grid, quiet_pairs, grid=True, per_cell=per_cell)),
             ("E  cell correlation", Calibrated(s_ncc, quiet_pairs, grid=True, per_cell=per_cell))]
    return cands


# ── the tests ────────────────────────────────────────────────────────────────


def pairs(frames):
    return list(zip(frames, frames[1:]))


def ae_steps(frames):
    out = []
    for p, c in pairs(frames):
        for g in (0.85, 1.15):
            out.append((p, np.clip(c * g, 0, 255)))
    return out


def small_person(person, quiet):
    """Person pairs shrunk to half size, pasted in the middle of a quiet pair."""
    out = []
    qp, qc = quiet[5], quiet[6]
    h, w = qp.shape
    for p, c in pairs(person):
        sp = np.asarray(Image.fromarray(p.astype(np.uint8)).resize((w // 2, h // 2)), dtype=np.float32)
        sc = np.asarray(Image.fromarray(c.astype(np.uint8)).resize((w // 2, h // 2)), dtype=np.float32)
        bp, bc = qp.copy(), qc.copy()
        y0, x0 = h // 4, w // 4
        bp[y0:y0 + h // 2, x0:x0 + w // 2] = sp
        bc[y0:y0 + h // 2, x0:x0 + w // 2] = sc
        out.append((bp, bc))
    return out


def tally(fn, prs):
    v = [fn(p, c) for p, c in prs]
    return {k: v.count(k) for k in ("quiet", "detect", "global")}


def run(data, CANDIDATES):
    tests = [
        ("baseline_still", pairs(data["baseline_still"]), "must be 0 detect"),
        ("ae_step (sim)", ae_steps(data["baseline_still"]), "must be 0 detect"),
        ("lamp on a lit person", pairs(data["light_change"]), "see note"),
        ("camera_nudge", pairs(data["camera_nudge"]), "few detect; global on the nudge"),
        ("person", pairs(data["person"]), "detect: more is better"),
        ("small_person (sim)", small_person(data["person"], data["baseline_still"]),
         "detect: more is better"),
    ]
    print(f"frames: " + ", ".join(f"{s} {len(data[s])}" for s in SETS)
          + f"; grid {GRID_X}x{GRID_Y}\n")
    head = "  " + "test".ljust(22) + "pairs".rjust(6) + "".join(n.split()[0].rjust(14) for n, _ in CANDIDATES)
    print(head + "\n  " + " " * 28 + "".join("detect/global".rjust(14) for _ in CANDIDATES))
    results = {}
    for tname, prs, want in tests:
        row = "  " + tname.ljust(22) + str(len(prs)).rjust(6)
        for cname, fn in CANDIDATES:
            t = tally(fn, prs)
            results[(tname, cname)] = t
            row += f"{t['detect']:>9}/{t['global']:<4}"
        print(row + f"   ({want})")
    print("\n  * A's thresholds were hand-set on this fixture, lamp and person included; every other\n"
          f"    threshold is {MARGIN}x the quiet set's maximum and saw no lamp, AE or person frame.")
    for n, fn in CANDIDATES:
        if hasattr(fn, "thr"):
            print(f"    {n:<22} threshold {fn.thr:.4f}" + (" (median over cells)" if getattr(fn, "grid", False) else ""))

    # The properties stated as checks, so the table cannot be misread.
    print()
    for cname, _ in CANDIDATES:
        fp = sum(results[(t, cname)]["detect"] for t in ("baseline_still", "ae_step (sim)"))
        hit = results[("person", cname)]["detect"]
        small = results[("small_person (sim)", cname)]["detect"]
        print(f"  {cname:<22} false detections on quiet + AE step: {fp:>3}   "
              f"person: {hit:>2}   small person: {small:>2}")


# ── recorded sessions (bench_events.py --record) ─────────────────────────────
#
# A recording is the real thing the host fixture and the simulations stand in
# for: the Senses, their own sensors and view, an empty room, a lamp with nobody
# in view, a person walking in. Each frame is labelled by the same code that
# labels the node's own runs (bench_events.label for lamp switches,
# the cue windows for walks), and every design is calibrated on that
# recording's own quiet phase -- which is what a node would do on install.

SKIP_WARMUP = 3  # the first frames after the port-open reset: stale, then AE converging


def load_session(d):
    import json
    import bench_events as be
    with open(os.path.join(d, "trace.jsonl"), encoding="utf-8") as fh:
        rows = [json.loads(l) for l in fh if l.strip()]
    with open(os.path.join(d, "cues.json"), encoding="utf-8") as fh:
        cue = json.load(fh)
    with open(os.path.join(d, "marks.json"), encoding="utf-8") as fh:
        marks = json.load(fh)
    rows = [r for r in rows if "file" in r]
    for r in rows:
        img = np.asarray(Image.open(os.path.join(d, r["file"])).convert("L"), dtype=np.float32)
        r["_img"], r["mean"] = img, float(img.mean())
    # A recording has no node verdicts, so the node's warm-up state is rebuilt
    # from the frames' own brightness with the node's rule (detector_math.rs
    # WarmUp: `ready` after SETTLE_RUN consecutive steps under SETTLE_DELTA).
    # The labeller needs it to know where a switch's settling ends; it is not
    # used to gate any candidate, which all score every frame.
    settle, run, last = be.settle_delta(), 0, None
    for r in rows:
        if last is None:
            r["state"] = "no_reference"
        else:
            run = run + 1 if abs(r["mean"] - last) < settle else 0
            r["state"] = "ready" if run >= 2 else "warming_up"
        last = r["mean"]
    if cue["mode"] == "lamp":
        be.label(rows, marks, settle, FRAC_HI)
    else:
        for r in rows:
            if r["phase"] != "person":
                r["tag"] = r["phase"]
                continue
            hit = [n for n, c in enumerate(cue["cues"]) if c <= r["t_sent"] < c + cue["walk_seconds"]]
            r["tag"] = "walk" if hit else "between"
            if hit:
                r["walk"] = hit[0]
    return rows, cue


def score_session(d, per_cell):
    rows, cue = load_session(d)
    quiet = [r for r in rows if r["tag"] == "quiet"][SKIP_WARMUP:]
    qpairs = [(a["_img"], b["_img"]) for a, b in zip(quiet, quiet[1:])]
    # The phase pairs: each frame against the one before it, across the phase
    # boundary too, exactly as the node would see them.
    phase = [k for k, r in enumerate(rows) if r["tag"] != "quiet"]
    cands = build(qpairs, per_cell)
    out = {}
    for name, fn in cands:
        v = {k: fn(rows[k - 1]["_img"], rows[k]["_img"]) for k in phase if k > 0}
        qv = [fn(a, b) for a, b in qpairs]
        res = {"quiet_detect": qv.count("detect")}
        if cue["mode"] == "lamp":
            for tag in ("switch", "unmarked", "resettle", "steady"):
                ks = [k for k in v if rows[k]["tag"] == tag]
                res[tag] = (sum(v[k] == "detect" for k in ks), sum(v[k] == "global" for k in ks), len(ks))
        else:
            walks = []
            for n, c in enumerate(cue["cues"]):
                ks = [k for k in v if rows[k].get("walk") == n]
                det = [k for k in ks if v[k] == "detect"]
                walks.append(round(rows[det[0]]["t_sent"] - c, 1) if det else None)
            res["walks"] = walks
            bs = [k for k in v if rows[k]["tag"] == "between"]
            res["between"] = (sum(v[k] == "detect" for k in bs), len(bs))
        out[name] = res
    return rows, cue, out


def report_session(d):
    for per_cell in (False, True):
        rows, cue, out = score_session(d, per_cell)
        node = os.path.basename(os.path.normpath(d))
        print("=" * 100)
        print(f"  {node}: {cue['mode']} recording, {len(rows)} frames; calibrated on its own quiet phase, "
              + ("one threshold per design" if not per_cell else "one threshold per cell"))
        print("=" * 100)
        if cue["mode"] == "lamp":
            print("  design                 quiet   switch det/glob/n   unmarked      resettle      steady"
                  "        (nobody in view: every detect is a false trigger)")
            for name, r in out.items():
                cells_ = "".join(f"  {r[t][0]:>3}/{r[t][1]:>3}/{r[t][2]:<4}" for t in ("switch", "unmarked", "resettle", "steady"))
                print(f"  {name:<22} {r['quiet_detect']:>4} {cells_}")
        else:
            print("  design                 quiet   walks detected   first detection, s after WALK NOW     between (false/n)")
            for name, r in out.items():
                w = r["walks"]
                got = sum(x is not None for x in w)
                lat = " ".join("-" if x is None else f"{x:.1f}" for x in w)
                print(f"  {name:<22} {r['quiet_detect']:>4}   {got:>2}/{len(w):<12}  {lat:<36}  {r['between'][0]}/{r['between'][1]}")
        print()


def main() -> int:
    if "--session" in sys.argv:
        dirs = sys.argv[sys.argv.index("--session") + 1:]
        if not dirs:
            print("usage: compare_detectors.py --session DIR [DIR ...]")
            return 2
        for d in dirs:
            report_session(d)
        return 0
    if not os.path.isdir(ROOT):
        print(f"fixture not found: {ROOT}")
        return 2
    data = {s: load_set(s) for s in SETS}
    for per_cell in (False, True):
        print("=" * 100)
        print("  calibration: " + ("ONE threshold per design (quiet maximum anywhere in the frame)"
                                   if not per_cell else "ONE threshold PER CELL (each cell's own quiet maximum)"))
        print("=" * 100)
        run(data, build(pairs(data["baseline_still"]), per_cell))
    return 0


if __name__ == "__main__":
    sys.exit(main())
