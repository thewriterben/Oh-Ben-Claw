"""Measure the on-node detector's quiet floor, with evidence that it was quiet.

    python scripts/vision/bench_floor.py <label> [count] [interval_s] [port]

Writes `obc-bench/floor/<label>/`:
    first.jpg, last.jpg   -- the scene at both ends of the run
    trace.jsonl           -- every `camera_detect` reply
    summary.json          -- the distribution, and what the host thresholds do to it

# Why the pictures

On 2026-09-17 a thirty-frame run showed `frac` running 0.11-0.23 where the host
fixture's empty-bench max was 0.083, and brightness wandering 84-119 where the
fixture held 108.7-111.0. That is exactly what the ADR predicted would happen
when the detector moved from host-decoded JPEG to raw Y8, and it was very nearly
written up as the confirmation.

It was not. A picture from the same session had a person in frame, moving. The
run measured a busy room, and a floor measured on a scene that is not quiet is
not a floor.

The detector could not have caught it: `warming_up` is what it says both when
auto-exposure is hunting and when someone is walking about. So this script takes
a picture at each end and keeps them next to the numbers. If the two pictures
disagree about what is in the room, the numbers between them are not a floor
either.

**COM10 by default, never COM6.**
"""

import base64
import json
import os
import statistics
import sys
import time

import serial

LABEL = sys.argv[1] if len(sys.argv) > 1 else "floor"
COUNT = int(sys.argv[2]) if len(sys.argv) > 2 else 200
INTERVAL = float(sys.argv[3]) if len(sys.argv) > 3 else 1.0
PORT = sys.argv[4] if len(sys.argv) > 4 else "COM10"

OUT = os.path.join(r"C:\Users\Benji\obc-bench\floor", LABEL)

# The host fixture's thresholds, which this run exists to test rather than apply.
FRAC_HI, EDGE_LIGHT, EDGE_NUDGE = 0.35, 4.20, 9.00
# The host fixture's own empty-bench maxima, for comparison.
HOST_BASELINE_FRAC_MAX, HOST_BASELINE_EDGE_MAX = 0.083, 3.40

if PORT.upper() == "COM6":
    sys.exit("refusing: COM6 is the live mesh node (obc-esp32-s3-001)")

os.makedirs(OUT, exist_ok=True)


def request(ser, ident, cmd, args, timeout=25):
    ser.write((json.dumps({"id": ident, "cmd": cmd, "args": args}) + "\n").encode())
    ser.flush()
    end = time.time() + timeout
    buf = b""
    while time.time() < end:
        c = ser.read(16384)
        if not c:
            continue
        buf += c
        if f'"{ident}"'.encode() in buf and buf.rstrip().endswith(b"}"):
            break
    for line in buf.decode("utf-8", "replace").splitlines():
        if f'"{ident}"' not in line:
            continue
        try:
            return json.loads(line)
        except Exception:
            continue
    return None


def snap(ser, ident, path):
    o = request(ser, ident, "camera_capture", {"quality": 8})
    if not o or not o.get("ok"):
        print(f"  !! no picture for {path}: {o.get('error') if o else 'no reply'}")
        return False
    data = base64.b64decode(o["result"])
    with open(path, "wb") as fh:
        fh.write(data)
    print(f"  wrote {path} ({len(data)} B)")
    return True


print(f"label={LABEL} count={COUNT} interval={INTERVAL}s port={PORT}", flush=True)
print(f"-> {OUT}\n", flush=True)

rows = []
with serial.Serial(PORT, 115200, timeout=0.3) as ser:
    time.sleep(3.5)  # the port open resets the board
    ser.reset_input_buffer()

    snap(ser, "snap0", os.path.join(OUT, "first.jpg"))

    t0 = time.time()
    for i in range(COUNT):
        o = request(ser, f"f{i}", "camera_detect", {}, timeout=12)
        if not o or not o.get("ok"):
            rows.append({"i": i, "error": (o or {}).get("error", "no reply")})
        else:
            r = o["result"]
            if isinstance(r, str):
                r = json.loads(r)
            r["i"] = i
            r["t"] = round(time.time() - t0, 2)
            rows.append(r)
        if (i + 1) % 25 == 0:
            ready = sum(1 for x in rows if x.get("state") == "ready")
            print(f"  {i + 1}/{COUNT}   judged {ready}", flush=True)
        time.sleep(INTERVAL)

    snap(ser, "snap1", os.path.join(OUT, "last.jpg"))

with open(os.path.join(OUT, "trace.jsonl"), "w", encoding="utf-8") as fh:
    for r in rows:
        fh.write(json.dumps(r) + "\n")

# ── the distribution ─────────────────────────────────────────────────────────
#
# Scored on every frame that HAS scores, not only the ones the warm-up gate let
# through. The gate decides what the node is willing to act on; the floor is a
# property of the pixels either way, and excluding frames the gate rejected
# would measure the gate.

scored = [r for r in rows if "frac" in r and "edge" in r]
means = [r["mean"] for r in rows if "mean" in r]


def pct(xs, p):
    if not xs:
        return None
    s = sorted(xs)
    return s[min(len(s) - 1, int(round(p / 100.0 * (len(s) - 1))))]


fr = [r["frac"] for r in scored]
ed = [r["edge"] for r in scored]

summary = {
    "label": LABEL,
    "frames": len(rows),
    "scored": len(scored),
    "judged": sum(1 for r in rows if r.get("state") == "ready"),
    "errors": sum(1 for r in rows if "error" in r),
    "interval_s": INTERVAL,
}

if scored:
    summary["frac"] = {
        "mean": round(statistics.fmean(fr), 4),
        "p95": round(pct(fr, 95), 4),
        "max": round(max(fr), 4),
        "host_baseline_max": HOST_BASELINE_FRAC_MAX,
    }
    summary["edge"] = {
        "mean": round(statistics.fmean(ed), 3),
        "p95": round(pct(ed, 95), 3),
        "max": round(max(ed), 3),
        "host_baseline_max": HOST_BASELINE_EDGE_MAX,
    }
if means:
    summary["brightness"] = {
        "min": round(min(means), 2),
        "max": round(max(means), 2),
        "spread": round(max(means) - min(means), 2),
    }

# The question the run is for: on a quiet bench, does the host rule stay silent?
false_positives = [
    r for r in scored if r["frac"] > FRAC_HI and r["edge"] > EDGE_LIGHT
]
summary["host_thresholds"] = {
    "frac_hi": FRAC_HI,
    "edge_light": EDGE_LIGHT,
    "edge_nudge": EDGE_NUDGE,
    "frames_the_rule_would_call_motion_or_nudge": len(false_positives),
    "of_scored": len(scored),
}

with open(os.path.join(OUT, "summary.json"), "w", encoding="utf-8") as fh:
    json.dump(summary, fh, indent=2)
    fh.write("\n")

print("\n" + json.dumps(summary, indent=2))
print(
    "\nLOOK AT first.jpg AND last.jpg BEFORE BELIEVING ANY OF THIS. "
    "If anything is moving in them, this is not a floor."
)
