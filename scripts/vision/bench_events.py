#!/usr/bin/env python3
"""Measure the on-node detector against lamp switches, with the switches marked.

    python scripts/vision/bench_events.py --port COM11 [--quiet 40] [--lamp-seconds 150]
    python scripts/vision/bench_events.py --port COM11 --mode person [--person-seconds 160]
    python scripts/vision/bench_events.py --replay results/vision-events/005-...
    python scripts/vision/bench_events.py --selftest

A guided session: a quiet phase, then a lamp phase in which you switch a lamp
roughly every 15 s and **press Enter the moment you flip it**. Every
`camera_detect` reply is kept with the wall clock, so each frame is labelled by
what the room was actually doing, not by what anyone remembers of it. That is
the lesson of the 2026-09-16 fixture, where a set label was wrong for two frames
and nothing could prove it afterwards.

# What it answers

The detector exists so that a lamp is never reported as a detection. On the
node that protection has two layers, and this measures both:

  * the **warm-up gate**: a lamp switch moves brightness by far more than
    SETTLE_DELTA, so the node says `warming_up` and gives no class at all
    until exposure settles again. `frames_to_ready` is how long that takes.
  * the **rule** (frac/edge thresholds from the host fixture): what it would
    call each frame on its own. A dimmer, daylight, or a lamp warming up does
    not step brightness, never trips the gate, and leaves the rule alone to
    decide. So the rule is scored on every frame that has scores, gated or not.

`--mode person` runs the same shape with WALK NOW cues: walk into view, across
and out. The cue times label the walks (no key to press mid-stride), and the
report says per walk whether the node detected it, how soon, and the two ways
a person could go unseen: the warm-up gate withholding judgement because a
body moved the brightness, and a `nudge` dropping the reference.

Camera-nudge is not measured by either mode. `thresholds_provisional` stays
true.

Writes results/vision-events/<node>-<stamp>/: first.jpg, last.jpg (the scene
at both ends -- look at them before believing a quiet phase was quiet),
trace.jsonl, marks.json, summary.json.
"""

from __future__ import annotations

import argparse
import base64
import datetime
import json
import math
import os
import re
import statistics
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))
LIVE_NODE = "obc-esp32-s3-001"
NODE = re.compile(r"obc-esp32-s3-[0-9a-z]{3,6}")


def thresholds() -> dict:
    """Read out of classify.py, as tests/firmware_detector_math.rs does, so no copy can drift."""
    with open(os.path.join(HERE, "classify.py"), encoding="utf-8") as fh:
        src = fh.read()
    out = {}
    for name in ("FRAC_HI", "EDGE_LIGHT", "EDGE_NUDGE", "PIXEL_EPS"):
        m = re.search(rf"^{name}\s*=\s*([0-9.]+)", src, re.M)
        if not m:
            raise SystemExit(f"{name} not found in classify.py -- it is the reference")
        out[name] = float(m.group(1))
    return out


def settle_delta() -> float:
    """The node's own SETTLE_DELTA, read out of detector_math.rs for the same reason."""
    path = os.path.join(ROOT, "firmware", "obc-esp32-s3", "src", "detector_math.rs")
    with open(path, encoding="utf-8") as fh:
        m = re.search(r"pub const SETTLE_DELTA: f32 = ([0-9.]+);", fh.read())
    if not m:
        raise SystemExit("SETTLE_DELTA not found in detector_math.rs")
    return float(m.group(1))


def rule(frac: float, edge: float, t: dict) -> str:
    """classify.py's rule, verbatim in meaning."""
    if frac <= t["FRAC_HI"]:
        return "quiet"
    if edge <= t["EDGE_LIGHT"]:
        return "light"
    if edge >= t["EDGE_NUDGE"]:
        return "nudge"
    return "motion"


# ── labelling ────────────────────────────────────────────────────────────────


WINDOW = 2  # frames either side of a mark to look for the brightness step


def label(rows: list[dict], marks: list[float], settle: float = 3.0,
          frac_hi: float = 0.35) -> list[dict]:
    """Give each lamp-phase frame a `tag`: switch, resettle or steady.

    A mark at time m first points at the first frame requested after it: the
    sensor captures when the request arrives, so that is the first frame that
    can have seen the new light. But a hand on a lamp and a finger on Enter are
    not simultaneous, so the switch frame is taken as the one with the largest
    brightness step within WINDOW frames of that, and `mark_offset` records how
    far it moved. WINDOW was 1 until the first real session (005, 2026-09-26):
    switch 1 there was pressed ~1.5 frames late, the labeller took the
    auto-exposure correction frame, and the flip itself (edge 4.71, the worst
    of the run) was filed as `steady`. Switches ~15 frames apart leave room. Frames after it
    are `resettle` until the node says `ready` again, then `steady`.

    The marks are a person's, and a person misses one or presses without
    flipping (both happened on 002's first session). So the labeller does not
    trust them blindly. A frame "changed" if brightness stepped by at least
    `settle` (what the gate sees) OR more than `frac_hi` of its pixels moved
    (what the rule sees) -- the second matters because a flip auto-exposure
    absorbed within one frame is exactly the one the gate misses. A mark with
    no changed frame in its window is returned as a **phantom** and makes no
    switch; a changed frame outside any switch's settling is tagged
    **`unmarked`** -- a switch nobody pressed Enter for -- rather than filed as
    steady, where it would poison the between-switch floor.
    """
    lamp = set(k for k, r in enumerate(rows) if r.get("phase") == "lamp")
    for r in rows:
        if r.get("phase") != "lamp":
            r["tag"] = r.get("phase", "?")

    def step(k):
        if k <= 0 or "mean" not in rows[k] or "mean" not in rows[k - 1]:
            return -1.0
        return abs(rows[k]["mean"] - rows[k - 1]["mean"])

    def changed(k):
        return step(k) >= settle or (rows[k].get("frac") or 0.0) > frac_hi

    switch_at: dict[int, tuple[float, int]] = {}
    phantoms: list[dict] = []
    for m in sorted(marks):
        k = next((k for k in sorted(lamp) if rows[k]["t_sent"] >= m), None)
        if k is None:
            continue  # marked after the last frame
        near = [c for c in range(k - WINDOW, k + WINDOW + 1) if c in lamp]
        best = max(near, key=lambda c: (changed(c), step(c), rows[c].get("frac") or 0.0, -abs(c - k)))
        if not changed(best):
            phantoms.append({"mark": m, "near_frame": rows[k].get("i"),
                             "largest_step": round(step(best), 2)})
            continue
        switch_at.setdefault(best, (m, best - k))

    settling = False
    for k in sorted(lamp):
        r = rows[k]
        if k in switch_at:
            r["tag"] = "switch"
            r["mark"], r["mark_offset"] = switch_at[k]
            settling = r.get("state") != "ready"
        elif settling:
            settling = r.get("state") != "ready"
            r["tag"] = "resettle" if settling else "steady"
        elif changed(k):
            r["tag"] = "unmarked"
            settling = r.get("state") != "ready"
        else:
            r["tag"] = "steady"
    return phantoms


# ── the report ───────────────────────────────────────────────────────────────


def pct(xs: list[float], p: float) -> float | None:
    if not xs:
        return None
    s = sorted(xs)
    return s[min(len(s) - 1, int(round(p / 100.0 * (len(s) - 1))))]


def floor_of(rows: list[dict]) -> dict:
    scored = [r for r in rows if "frac" in r and "edge" in r]
    means = [r["mean"] for r in rows if "mean" in r]
    out = {"frames": len(rows), "scored": len(scored),
           "judged": sum(1 for r in rows if r.get("state") == "ready")}
    if scored:
        fr, ed = [r["frac"] for r in scored], [r["edge"] for r in scored]
        out["frac"] = {"p95": round(pct(fr, 95), 4), "max": round(max(fr), 4)}
        out["edge"] = {"p95": round(pct(ed, 95), 3), "max": round(max(ed), 3)}
    if means:
        out["brightness"] = {"min": round(min(means), 2), "max": round(max(means), 2)}
    return out


def summarise(rows: list[dict], marks: list[float], t: dict, settle: float = 3.0) -> dict:
    phantoms = label(rows, marks, settle, t["FRAC_HI"])
    for r in rows:
        if "frac" in r and "edge" in r:
            r["rule"] = rule(r["frac"], r["edge"], t)

    quiet = [r for r in rows if r.get("tag") == "quiet"]
    lamp = [r for r in rows if r.get("phase") == "lamp"]
    switches = []
    for k, r in enumerate(rows):
        if r.get("tag") not in ("switch", "unmarked"):
            continue
        before = next((x.get("mean") for x in reversed(rows[:k]) if "mean" in x), None)
        n = 0
        for x in rows[k:]:
            if x.get("state") == "ready":
                break
            n += 1
        else:
            n = None  # never settled before the run ended
        switches.append({
            "i": r.get("i"), "state": r.get("state"), "node_class": r.get("class"),
            "frac": r.get("frac"), "edge": r.get("edge"), "rule": r.get("rule"),
            "mean_before": before, "mean_after": r.get("mean"), "frames_to_ready": n,
            "mark_offset": r.get("mark_offset"), "marked": r.get("tag") == "switch",
        })

    def detections(xs):
        return sum(1 for x in xs if x.get("detection") is True)

    def rule_hits(xs):
        return sum(1 for x in xs if x.get("rule") in ("motion", "nudge"))

    steady = [r for r in lamp if r.get("tag") == "steady"]
    return {
        "node_id": next((r.get("node_id") for r in rows if r.get("node_id")), None),
        "thresholds": t,
        "marks": len(marks),
        "quiet": {**floor_of(quiet), "node_detections": detections(quiet),
                  "rule_motion_or_nudge": rule_hits(quiet)},
        "lamp": {
            "frames": len(lamp),
            "node_detections": detections(lamp),
            "node_classes": {c: sum(1 for x in lamp if x.get("class") == c)
                             for c in sorted({x.get("class") for x in lamp if x.get("class")})},
            "rule_on_switch_frames": {c: sum(1 for s in switches if s["rule"] == c)
                                      for c in sorted({s["rule"] for s in switches if s["rule"]})},
            "rule_motion_or_nudge_any_frame": rule_hits(lamp),
            "switches_the_gate_held": sum(1 for s in switches if s["state"] != "ready"),
            "steady": floor_of(steady),
        },
        "switches": switches,
        "phantom_marks": phantoms,
        "unmarked_switches": sum(1 for w in switches if not w["marked"]),
        "settle_delta": settle,
        "errors": sum(1 for r in rows if "error" in r),
    }


def print_report(s: dict) -> None:
    q, l = s["quiet"], s["lamp"]
    print(f"\n=== {s['node_id']}  ({s['marks']} marks, {len(s['switches'])} switches found) ===")
    print(f"quiet  {q['frames']} frames, {q['judged']} judged; "
          f"frac max {q.get('frac', {}).get('max')}  edge max {q.get('edge', {}).get('max')}  "
          f"brightness {q.get('brightness')}")
    print(f"       node detections {q['node_detections']}   rule motion/nudge {q['rule_motion_or_nudge']}")
    print(f"\n  switch   state        node    frac     edge   rule    mean before->after  to ready  mark")
    for w in s["switches"]:
        mb = f"{w['mean_before']:.1f}" if w["mean_before"] is not None else "?"
        ma = f"{w['mean_after']:.1f}" if w["mean_after"] is not None else "?"
        fr = f"{w['frac']:.4f}" if w["frac"] is not None else "-"
        ed = f"{w['edge']:.2f}" if w["edge"] is not None else "-"
        print(f"  {w['i']!s:>6}   {w['state']!s:<12} {w['node_class'] or '-':<7} {fr:>7} {ed:>8}   "
              f"{w['rule'] or '-':<7} {mb:>7} -> {ma:<7}   {w['frames_to_ready']!s:>8}  "
              + (f"{w['mark_offset']:+d}" if w["marked"] else "UNMARKED"))
    print(f"\nlamp   {l['frames']} frames; node detections {l['node_detections']}; "
          f"node classes {l['node_classes']}")
    print(f"       gate held {l['switches_the_gate_held']}/{len(s['switches'])} switch frames; "
          f"rule alone on switch frames {l['rule_on_switch_frames']}; "
          f"rule motion/nudge on any lamp frame {l['rule_motion_or_nudge_any_frame']}")
    for p in s["phantom_marks"]:
        print(f"       PHANTOM MARK near frame {p['near_frame']}: nothing changed within {WINDOW} frames "
              f"(largest brightness step {p['largest_step']}, no frac > FRAC_HI) -- not counted")
    if s["unmarked_switches"]:
        print(f"       {s['unmarked_switches']} UNMARKED switch(es): a brightness step with no Enter near it")
    st = l["steady"]
    print(f"       steady-state under each lamp setting: frac max {st.get('frac', {}).get('max')}  "
          f"edge max {st.get('edge', {}).get('max')}")
    verdict = ("NO DETECTIONS from lamp switches" if l["node_detections"] == 0 and q["node_detections"] == 0
               else "THE NODE REPORTED A DETECTION -- see trace.jsonl")
    print(f"\n  {verdict}.  Look at first.jpg and last.jpg before believing the quiet phase.")
    if s["errors"]:
        print(f"  {s['errors']} frame(s) had no reply or an error.")


# ── the person phase ─────────────────────────────────────────────────────────


def summarise_person(rows: list[dict], cues: list[float], walk_s: float, t: dict) -> dict:
    """Score walks through the frame, one window per cue.

    A person cannot press Enter while crossing the room, so the script's own
    cue times are the labels: the `walk_s` seconds after each `WALK NOW` are a
    walk window, and every person-phase frame outside one is `between`. The
    walker is in view for part of each window, not all of it, so the question
    answered per walk is "did the node detect it, and how soon" -- not a
    frame-level recall, which these labels cannot support.

    The two ways a person could go unseen are counted, not assumed away:
    `gate_withheld` (the frame changed -- `frac` over `FRAC_HI` -- but the
    warm-up gate gave no class, because a body in frame moved the brightness)
    and `nudge` (a judged frame scored `nudge`, which drops the reference and
    blinds the node for the frames after).
    """
    for r in rows:
        if "frac" in r and "edge" in r:
            r["rule"] = rule(r["frac"], r["edge"], t)
        if r.get("phase") != "person":
            r["tag"] = r.get("phase", "?")
            continue
        hit = [n for n, c in enumerate(cues) if c <= r["t_sent"] < c + walk_s]
        r["tag"] = "walk" if hit else "between"
        if hit:
            r["walk"] = hit[0]

    walks = []
    for n, c in enumerate(cues):
        w = [r for r in rows if r.get("walk") == n]
        det = [r for r in w if r.get("detection") is True]
        scored = [r for r in w if "frac" in r]
        walks.append({
            "walk": n + 1, "frames": len(w),
            "judged": sum(1 for r in w if r.get("state") == "ready"),
            "node_classes": {k: sum(1 for r in w if r.get("class") == k)
                             for k in sorted({r.get("class") for r in w if r.get("class")})},
            "detected": bool(det),
            "first_detection_s": round(det[0]["t_sent"] - c, 1) if det else None,
            "gate_withheld": sum(1 for r in w if r.get("state") != "ready"
                                 and (r.get("frac") or 0) > t["FRAC_HI"]),
            "nudge": sum(1 for r in w if r.get("class") == "nudge"),
            "rule_alone": {k: sum(1 for r in scored if r.get("rule") == k)
                           for k in sorted({r.get("rule") for r in scored})},
            "frac_max": round(max((r["frac"] for r in scored), default=0), 4),
            "edge_max": round(max((r["edge"] for r in scored), default=0), 2),
        })
    between = [r for r in rows if r.get("tag") == "between"]
    quiet = [r for r in rows if r.get("tag") == "quiet"]
    # A walker slow to leave frame spills past the window; timing each
    # between-walks detection against the last cue lets a reader tell that
    # tail (~walk_s + a few s) from a detection with nobody there.
    spill = []
    for r in between:
        if r.get("detection") is True:
            before = [c for c in cues if c <= r["t_sent"]]
            spill.append({"i": r.get("i"),
                          "after_cue_s": round(r["t_sent"] - before[-1], 1) if before else None})
    return {
        "mode": "person",
        "node_id": next((r.get("node_id") for r in rows if r.get("node_id")), None),
        "thresholds": t, "walk_seconds": walk_s,
        "quiet": {**floor_of(quiet),
                  "node_detections": sum(1 for r in quiet if r.get("detection") is True)},
        "walks": walks,
        "walks_detected": sum(1 for w in walks if w["detected"]),
        "between": {**floor_of(between),
                    "node_detections": sum(1 for r in between if r.get("detection") is True),
                    "detections_after_cue_s": spill,
                    "node_classes": {k: sum(1 for r in between if r.get("class") == k)
                                     for k in sorted({r.get("class") for r in between if r.get("class")})}},
        "errors": sum(1 for r in rows if "error" in r),
    }


def print_person_report(s: dict) -> None:
    q, b = s["quiet"], s["between"]
    print(f"\n=== {s['node_id']}  ({len(s['walks'])} walks, {s['walk_seconds']:.0f} s window each) ===")
    print(f"quiet    {q['frames']} frames, {q['judged']} judged; frac max {q.get('frac', {}).get('max')}  "
          f"edge max {q.get('edge', {}).get('max')}; node detections {q['node_detections']}")
    print("\n  walk  frames judged  detected  first(s)  gate-withheld  nudge  frac max  edge max  "
          "node classes / rule alone")
    for w in s["walks"]:
        print(f"  {w['walk']:>4}  {w['frames']:>6} {w['judged']:>6}  {('YES' if w['detected'] else 'no'):>8}  "
              f"{w['first_detection_s'] if w['first_detection_s'] is not None else '-':>8}  "
              f"{w['gate_withheld']:>13}  {w['nudge']:>5}  {w['frac_max']:>8}  {w['edge_max']:>8}  "
              f"{w['node_classes']} / {w['rule_alone']}")
    print(f"\nbetween walks  {b['frames']} frames; node detections {b['node_detections']}; "
          f"classes {b['node_classes']}")
    if b["detections_after_cue_s"]:
        when = ", ".join(f"{d['after_cue_s']} s" for d in b["detections_after_cue_s"])
        print(f"               seconds after the last WALK NOW: {when}\n"
              f"               (just past {s['walk_seconds']:.0f} s is a walker still leaving; "
              "long after is a false positive)")
    print(f"\n  {s['walks_detected']}/{len(s['walks'])} walks detected.  Look at first.jpg and "
          "last.jpg: the room must be empty at both ends.")
    if s["errors"]:
        print(f"  {s['errors']} frame(s) had no reply or an error.")


# ── the live run ─────────────────────────────────────────────────────────────


class Link:
    def __init__(self, port: str):
        import serial

        self.ser = serial.Serial(port, 115200, timeout=0.3)
        time.sleep(3.5)  # opening the port may reset the board
        self.ser.reset_input_buffer()

    def request(self, ident: str, cmd: str, args: dict, timeout: float = 12.0) -> dict | None:
        self.ser.write((json.dumps({"id": ident, "cmd": cmd, "args": args}) + "\n").encode())
        self.ser.flush()
        end, buf = time.time() + timeout, b""
        while time.time() < end:
            chunk = self.ser.read(16384)
            if not chunk:
                continue
            buf += chunk
            if f'"{ident}"'.encode() in buf and buf.rstrip().endswith(b"}"):
                break
        for line in buf.decode("utf-8", "replace").splitlines():
            if f'"{ident}"' not in line or "{" not in line:
                continue
            try:
                obj = json.loads(line[line.index("{"):])
            except ValueError:
                continue
            if isinstance(obj, dict) and str(obj.get("id")) == ident:
                return obj
        return None

    def snap(self, ident: str, path: str) -> None:
        o = self.request(ident, "camera_capture", {"quality": 8}, timeout=25)
        if not o or not o.get("ok"):
            print(f"  !! no picture for {os.path.basename(path)}: {o.get('error') if o else 'no reply'}")
            return
        with open(path, "wb") as fh:
            fh.write(base64.b64decode(o["result"]))
        print(f"  wrote {os.path.basename(path)}")


def frame(link: Link, i: int, phase: str, t0: float) -> dict:
    sent = time.time()
    o = link.request(f"f{i}", "camera_detect", {})
    if not o or not o.get("ok"):
        row = {"error": (o or {}).get("error", "no reply")}
    else:
        row = o["result"] if isinstance(o["result"], dict) else json.loads(o["result"])
    row.update({"i": i, "phase": phase, "t_sent": sent, "t": round(sent - t0, 2)})
    return row


def live(args, t: dict) -> str:
    link = Link(args.port)
    who = link.request("who", "capabilities", {}, timeout=5)
    m = NODE.search(json.dumps(who)) if who else None
    node = m.group(0) if m else None
    if node == LIVE_NODE:
        sys.exit(f"{args.port} is {LIVE_NODE}, the live mesh node. No camera command sent.")
    short = (node or "unknown").replace("obc-esp32-s3-", "")
    out = os.path.join(ROOT, "results", "vision-events",
                       f"{short}-{datetime.datetime.now().strftime('%Y%m%d-%H%M%S')}")
    os.makedirs(out, exist_ok=True)
    print(f"{args.port}: {node or 'node unidentified'}  ->  {out}")

    marks: list[float] = []
    armed = threading.Event()
    lamp = args.mode == "lamp"

    def watch_enter():
        for _ in sys.stdin:
            if armed.is_set():
                marks.append(time.time())
                print(f"    [{'switch' if lamp else 'mark'} {len(marks)} marked]", flush=True)

    threading.Thread(target=watch_enter, daemon=True).start()

    start_state = "Lamp in its starting state, and get" if lamp else "Get"
    print(f"\n{start_state} out of shot. First picture in {args.lead:.0f} s.")
    time.sleep(args.lead)
    link.snap("snap0", os.path.join(out, "first.jpg"))

    rows, t0, i = [], time.time(), 0
    print(f"\nQUIET: {args.quiet} frames. Nothing moves" + (", lamp stays as it is." if lamp else "."))
    for _ in range(args.quiet):
        rows.append(frame(link, i, "quiet", t0))
        i += 1
        if i % 10 == 0:
            print(f"  {i}/{args.quiet}", flush=True)
        time.sleep(args.interval)

    every = args.every or (15.0 if lamp else 20.0)
    seconds = args.lamp_seconds if lamp else args.person_seconds
    if lamp:
        print(f"\nLAMP: {seconds:.0f} s. Every {every:.0f} s the script counts down "
              "3, 2, 1 -- on SWITCH NOW, flip the lamp and press ENTER.\n"
              "Exact timing does not matter: a press within ~2 s of the flip is matched to it,\n"
              "and a missed or extra press is reported, not guessed. Stay out of shot.")
        verb = "SWITCH"
    else:
        print(f"\nPERSON: {seconds:.0f} s. Every {every:.0f} s the script counts down 3, 2, 1 -- "
              "on WALK NOW, walk into view,\n"
              f"across the frame at a normal pace and back out, within about {args.walk_seconds:.0f} s. "
              "No key to press.\nBetween walks, stay out of shot and keep still.")
        verb = "WALK"
    armed.set()
    start = time.time()
    end = start + seconds
    # Cues are spaced from the start of the phase, the first one `every` s in, so
    # the scene has sat still long enough to settle. The last cue is kept clear
    # of the end so what it starts has frames to finish in.
    tail = 5 if lamp else args.walk_seconds + 3
    cues = [start + every * n for n in range(1, int(seconds // every) + 1)
            if start + every * n <= end - tail]
    said: set[tuple[int, int]] = set()
    while time.time() < end:
        rows.append(frame(link, i, args.mode, t0))
        i += 1
        now = time.time()
        # A frame takes ~1.3 s at the default pace, so a count is printed when
        # its second has arrived rather than only inside a 1 s window that a
        # slow frame could step over; NOW is printed once, however late.
        for n, c in enumerate(cues):
            left = c - now
            if 0 < left <= 3:
                count = math.ceil(left)
                if (n, count) not in said:
                    said.add((n, count))
                    print(f"  {verb.lower()} {n + 1}/{len(cues)} in {count}", flush=True)
            elif left <= 0 and (n, 0) not in said:
                said.add((n, 0))
                print(f"  >>> {verb} NOW  ({n + 1}/{len(cues)})", flush=True)
        time.sleep(args.interval)
    armed.clear()

    with open(os.path.join(out, "cues.json"), "w", encoding="utf-8") as fh:
        json.dump({"mode": args.mode, "cues": cues, "walk_seconds": args.walk_seconds}, fh)
    link.snap("snap1", os.path.join(out, "last.jpg"))
    with open(os.path.join(out, "trace.jsonl"), "w", encoding="utf-8") as fh:
        for r in rows:
            fh.write(json.dumps(r) + "\n")
    with open(os.path.join(out, "marks.json"), "w", encoding="utf-8") as fh:
        json.dump(marks, fh)
    return out


def replay(out: str, t: dict) -> dict:
    with open(os.path.join(out, "trace.jsonl"), encoding="utf-8") as fh:
        rows = [json.loads(l) for l in fh if l.strip()]
    with open(os.path.join(out, "marks.json"), encoding="utf-8") as fh:
        marks = json.load(fh)
    cues_path = os.path.join(out, "cues.json")
    cue = {}
    if os.path.isfile(cues_path):  # lamp runs before 2026-09-26 evening have none
        with open(cues_path, encoding="utf-8") as fh:
            cue = json.load(fh)
    if cue.get("mode") == "person":
        s = summarise_person(rows, cue["cues"], cue["walk_seconds"], t)
    else:
        s = summarise(rows, marks, t, settle_delta())
    with open(os.path.join(out, "summary.json"), "w", encoding="utf-8") as fh:
        json.dump(s, fh, indent=2)
        fh.write("\n")
    return s


# ── selftest ─────────────────────────────────────────────────────────────────


def selftest() -> int:
    t = thresholds()
    fails = []
    if t != {"FRAC_HI": 0.35, "EDGE_LIGHT": 4.2, "EDGE_NUDGE": 9.0, "PIXEL_EPS": 12.0}:
        fails.append(f"thresholds read from classify.py: {t}")
    for (fr, ed), want in {(0.2, 9.9): "quiet", (0.9, 3.0): "light", (0.9, 6.0): "motion",
                           (0.9, 12.0): "nudge", (0.35, 20.0): "quiet"}.items():
        if rule(fr, ed, t) != want:
            fails.append(f"rule({fr}, {ed}) != {want}")

    def r(i, phase, ts, state, frac=0.0, edge=0.6, mean=120.0, **kw):
        return {"i": i, "phase": phase, "t_sent": ts, "state": state, "frac": frac,
                "edge": edge, "mean": mean, "node_id": "obc-esp32-s3-005", **kw}

    rows = [r(0, "quiet", 0, "no_reference"),
            r(1, "quiet", 1, "ready", **{"class": "quiet", "detection": False}),
            r(2, "lamp", 2, "ready", **{"class": "quiet", "detection": False}),
            # switch marked at 2.5 -> belongs to frame 3 (sent at 3)
            r(3, "lamp", 3, "warming_up", frac=0.8, edge=3.1, mean=60.0),
            r(4, "lamp", 4, "warming_up", frac=0.1, edge=0.7, mean=58.0),
            r(5, "lamp", 5, "ready", mean=58.5, **{"class": "quiet", "detection": False}),
            r(6, "lamp", 6, "ready", mean=58.4, **{"class": "quiet", "detection": False}),
            # a switch the gate did NOT hold, and the rule calls it light
            r(7, "lamp", 7, "ready", frac=0.6, edge=3.5, mean=60.0, **{"class": "light", "detection": False})]
    s = summarise(rows, [2.5, 6.5], t)
    tags = [x.get("tag") for x in rows]
    if tags != ["quiet", "quiet", "steady", "switch", "resettle", "steady", "steady", "switch"]:
        fails.append(f"tags {tags}")
    sw = s["switches"]
    if [w["frames_to_ready"] for w in sw] != [2, 0]:
        fails.append(f"frames_to_ready {[w['frames_to_ready'] for w in sw]}")
    if s["lamp"]["switches_the_gate_held"] != 1:
        fails.append(f"gate held {s['lamp']['switches_the_gate_held']}")
    if s["lamp"]["rule_on_switch_frames"] != {"light": 2}:
        fails.append(f"rule on switches {s['lamp']['rule_on_switch_frames']}")
    if s["lamp"]["node_detections"] != 0 or s["quiet"]["node_detections"] != 0:
        fails.append("phantom detection")
    if sw[0]["mean_before"] != 120.0 or sw[0]["mean_after"] != 60.0:
        fails.append(f"brightness step {sw[0]['mean_before']} -> {sw[0]['mean_after']}")
    # A late finger: the lamp flipped before frame 3 but Enter landed after it.
    late = [dict(x) for x in rows]
    s_late = summarise(late, [3.4, 6.5], t)
    if [w["i"] for w in s_late["switches"]] != [3, 7] or s_late["switches"][0]["mark_offset"] != -1:
        fails.append(f"late mark not pulled back to the step: {[(w['i'], w['mark_offset']) for w in s_late['switches']]}")
    # Two frames late, as switch 1 of the first real session was.
    s_2late = summarise([dict(x) for x in rows], [4.4, 6.5], t)
    if [w["i"] for w in s_2late["switches"]][:1] != [3]:
        fails.append(f"a mark two frames late was not pulled back: {[w['i'] for w in s_2late['switches']]}")
    # A phantom press (no step near it) and a flip nobody marked.
    ph = [dict(x) for x in rows] + [r(8, "lamp", 8, "ready", mean=60.1, **{"class": "quiet", "detection": False}),
                                   r(9, "lamp", 9, "ready", mean=60.0, **{"class": "quiet", "detection": False}),
                                   r(10, "lamp", 10, "ready", mean=60.1, **{"class": "quiet", "detection": False}),
                                   r(11, "lamp", 11, "ready", mean=60.0, **{"class": "quiet", "detection": False}),
                                   r(12, "lamp", 12, "ready", mean=60.1, **{"class": "quiet", "detection": False}),
                                   r(13, "lamp", 13, "warming_up", frac=0.7, edge=12.0, mean=110.0)]
    s_ph = summarise(ph, [2.5, 6.5, 9.5], t)
    if len(s_ph["phantom_marks"]) != 1 or s_ph["unmarked_switches"] != 1:
        fails.append(f"phantom {s_ph['phantom_marks']} unmarked {s_ph['unmarked_switches']}")
    if ph[13].get("tag") != "unmarked" or s_ph["lamp"]["steady"].get("edge", {}).get("max", 0) > 1:
        fails.append(f"unmarked step leaked into steady: tag {ph[13].get('tag')}")
    # Person mode: cue at 10, 8 s windows. Walk 1 detected on its second frame
    # after one gate-withheld frame; walk 2 missed with a nudge; one false
    # positive between walks.
    def p(i, ts, state, frac, edge, **kw):
        return r(i, "person", ts, state, frac=frac, edge=edge, **kw)
    prow = [r(0, "quiet", 0, "ready", **{"class": "quiet", "detection": False}),
            p(1, 9, "ready", 0.01, 1.0, **{"class": "quiet", "detection": False}),
            p(2, 10.5, "warming_up", 0.6, 6.0),
            p(3, 12, "ready", 0.5, 6.0, **{"class": "motion", "detection": True}),
            p(4, 19, "ready", 0.01, 1.0, **{"class": "quiet", "detection": False}),
            p(5, 25, "ready", 0.6, 3.0, **{"class": "light", "detection": False}),
            p(6, 30, "ready", 0.9, 11.0, **{"class": "nudge", "detection": False}),
            p(7, 34, "ready", 0.5, 6.0, **{"class": "motion", "detection": True})]
    sp = summarise_person(prow, [10.0, 30.0], 3.0, t)
    w1, w2 = sp["walks"]
    if not (w1["detected"] and w1["first_detection_s"] == 2.0 and w1["gate_withheld"] == 1):
        fails.append(f"walk 1 {w1}")
    if w2["detected"] or w2["nudge"] != 1:
        fails.append(f"walk 2 {w2}")
    if sp["walks_detected"] != 1 or sp["between"]["node_detections"] != 1:
        fails.append(f"person totals {sp['walks_detected']} / between {sp['between']['node_detections']}")
    rows2 = [dict(x) for x in rows]
    rows2[7]["detection"] = True
    if summarise(rows2, [2.5, 6.5], t)["lamp"]["node_detections"] != 1:
        fails.append("a node detection during the lamp phase was not counted")
    if fails:
        print("SELFTEST FAILED\n  " + "\n  ".join(fails))
        return 1
    print("selftest ok: thresholds read from classify.py, rule, switch/resettle/steady labelling, "
          "frames_to_ready, gate-held count, detections counted; person windows, "
          "gate-withheld, nudge, latency, false positives")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--port")
    ap.add_argument("--quiet", type=int, default=40, help="frames in the quiet phase")
    ap.add_argument("--lamp-seconds", type=float, default=150.0)
    ap.add_argument("--interval", type=float, default=1.0)
    ap.add_argument("--mode", choices=("lamp", "person"), default="lamp")
    ap.add_argument("--every", type=float, default=None,
                    help="seconds between cues (default 15 for lamp, 20 for person)")
    ap.add_argument("--person-seconds", type=float, default=160.0)
    ap.add_argument("--walk-seconds", type=float, default=8.0,
                    help="how long after WALK NOW a frame counts as part of the walk")
    ap.add_argument("--lead", type=float, default=15.0, help="seconds to get out of shot")
    ap.add_argument("--replay", metavar="DIR", help="re-score a saved run")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    t = thresholds()
    if args.replay:
        out = args.replay
    elif args.port:
        out = live(args, t)
    else:
        ap.error("--port is required (scripts/which_esp32.ps1 names it), or --replay DIR")
    s = replay(out, t)
    (print_person_report if s.get("mode") == "person" else print_report)(s)
    print(f"\n  wrote {os.path.join(out, 'summary.json')}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
