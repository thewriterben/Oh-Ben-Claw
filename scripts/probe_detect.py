"""Run the on-node detector and print what it says, frame by frame.

    python scripts/probe_detect.py [count] [interval_s] [port]

This is the instrument the 2026-09-17 ADR asks for. The three thresholds in
`detector_math.rs` were measured on a host, against frames that had been through
the sensor's JPEG encoder and a host-side decode. This node differences the
sensor's raw Y8 and never encodes anything, so it is looking at different
pixels -- no chroma subsampling, no DCT ringing, and `edge` is precisely the
score that would notice ringing.

Whether that moves the numbers a little or a lot is not known. This prints
`frac`, `edge` and `mean` per frame so it can be found out. **Until it has been,
the node's `class` is provisional and the reply says so.**

What to look for on an empty bench:
  * the first reply is `no_reference` -- there is nothing to compare to
  * then `warming_up` for a frame or two, while auto-exposure settles
  * then `ready`, and `frac` should sit near the floor

`warming_up` is not a failure. A detector armed at boot calls the auto-exposure
convergence a major event every single boot; withholding judgement and SAYING so
is the point.

**COM10 by default, never COM6.**
"""

import json
import sys
import time

import serial

COUNT = int(sys.argv[1]) if len(sys.argv) > 1 else 10
INTERVAL = float(sys.argv[2]) if len(sys.argv) > 2 else 1.0
PORT = sys.argv[3] if len(sys.argv) > 3 else "COM10"

if PORT.upper() == "COM6":
    sys.exit("refusing: COM6 is the live mesh node (obc-esp32-s3-001)")

print(f"port={PORT} count={COUNT} interval={INTERVAL}s\n", flush=True)
print("   #  state        class    detect     frac     edge     mean", flush=True)

rows = []
with serial.Serial(PORT, 115200, timeout=0.3) as ser:
    time.sleep(3.5)  # the port open resets the board
    ser.reset_input_buffer()
    for i in range(COUNT):
        req = {"id": f"x{i}", "cmd": "camera_detect", "args": {}}
        ser.write((json.dumps(req) + "\n").encode())
        ser.flush()
        end = time.time() + 12
        obj = None
        while time.time() < end and obj is None:
            raw = ser.readline()
            if not raw:
                continue
            line = raw.decode("utf-8", "replace").strip()
            if f'"x{i}"' not in line:
                continue
            try:
                obj = json.loads(line)
            except Exception:
                obj = {"ok": False, "error": line[:160]}
        if obj is None:
            print(f"  {i:2d}  NO REPLY")
            continue
        if not obj.get("ok"):
            print(f"  {i:2d}  FAIL {obj.get('error')}")
            continue
        # The node returns its payload as a JSON string in `result`.
        r = obj["result"]
        if isinstance(r, str):
            r = json.loads(r)
        rows.append(r)
        print(
            f"  {i:2d}  {r.get('state',''):<12} {r.get('class','-'):<8} "
            f"{str(r.get('detection','-')):<8} "
            f"{r.get('frac','-'):>8} {r.get('edge','-'):>8} {r.get('mean','-'):>8}"
        )
        if "why_no_class" in r:
            print(f"        ({r['why_no_class']})")
        time.sleep(INTERVAL)

if not rows:
    sys.exit("no replies at all")

ready = [r for r in rows if r.get("state") == "ready"]
print(f"\n  {len(rows)} replies, {len(ready)} of them judged")
if ready:
    fr = [r["frac"] for r in ready if "frac" in r]
    ed = [r["edge"] for r in ready if "edge" in r]
    if fr:
        print(f"  frac  min {min(fr):.4f}  max {max(fr):.4f}")
        print(f"  edge  min {min(ed):.2f}  max {max(ed):.2f}")
    classes = {}
    for r in ready:
        classes[r.get("class", "?")] = classes.get(r.get("class", "?"), 0) + 1
    print(f"  classes: {classes}")

if rows and rows[0].get("state") != "no_reference":
    print("\n  NOTE: the first reply was not `no_reference` -- the node kept a "
          "reference frame across the port-open reset, which it should not have.")

provisional = [r for r in rows if not r.get("thresholds_provisional")]
if provisional:
    sys.exit(
        "\n  the node stopped marking its thresholds provisional. That flag comes "
        "off when they are re-measured HERE, not before."
    )
sys.exit(0)
