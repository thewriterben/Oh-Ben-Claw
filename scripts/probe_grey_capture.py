"""Measure what one greyscale capture actually produces on the camera node.

    python scripts/probe_grey_capture.py [count] [port]

This is the check the 2026-09-17 ADR ("The node captures greyscale and only
greyscale", OBC-Prime docs/DECISIONS.md) has to survive. That entry argues from
a reading of esp32-camera v2.0.7 that `PIXFORMAT_GRAYSCALE` at QVGA yields a
76,800-byte Y8 frame. Reading a driver is evidence about the driver, not about
this board -- so this prints what the node says and compares it to the claim.

It reads the node's own console line

    capture: frame len=N B, WxH, format=F, base64 will be ~M B

rather than the command reply, because the reply is deliberately refused while
the frame is not a JPEG (see camera.rs). The absence of a reply is expected
here; the absence of that log line is not.

**COM10 by default, never COM6.** COM6 is the live mesh node -- run
`scripts/which_esp32.ps1` first and pass the port it names.
"""

import json
import re
import sys
import time

import serial

PORT = sys.argv[2] if len(sys.argv) > 2 else "COM10"
COUNT = int(sys.argv[1]) if len(sys.argv) > 1 else 3

# What the ADR predicts for QVGA greyscale. Stated here so a mismatch is loud
# rather than something you have to notice in a wall of output.
EXPECT_LEN = 320 * 240
EXPECT_WH = (320, 240)

if PORT.upper() == "COM6":
    sys.exit("refusing: COM6 is the live mesh node (obc-esp32-s3-001)")

CAP = re.compile(
    r"capture: frame len=(\d+) B, (\d+)x(\d+), format=(\d+)"
)

print(f"port={PORT} count={COUNT}", flush=True)
print(f"expecting len={EXPECT_LEN} at {EXPECT_WH[0]}x{EXPECT_WH[1]}\n", flush=True)

seen = []
with serial.Serial(PORT, 115200, timeout=0.3) as ser:
    time.sleep(3.5)  # the port open resets the board; let it boot
    ser.reset_input_buffer()
    for i in range(COUNT):
        req = {"id": f"g{i}", "cmd": "camera_capture", "args": {"quality": 10}}
        ser.write((json.dumps(req) + "\n").encode())
        ser.flush()
        end = time.time() + 12
        hit = None
        err = None
        while time.time() < end and hit is None:
            raw = ser.readline()
            if not raw:
                continue
            line = raw.decode("utf-8", "replace").strip()
            m = CAP.search(line)
            if m:
                hit = tuple(int(g) for g in m.groups())
                continue
            if f'"g{i}"' in line:
                try:
                    err = json.loads(line).get("error")
                except Exception:
                    err = line[:120]
        if hit is None:
            print(f"  {i:02d} NO CAPTURE LOG -- the sensor gave no frame, or the "
                  f"log line moved")
            seen.append(None)
            continue
        length, w, h, fmt = hit
        ok = length == EXPECT_LEN and (w, h) == EXPECT_WH
        print(f"  {i:02d} len={length} {w}x{h} format={fmt}  {'ok' if ok else 'MISMATCH'}")
        if err:
            print(f"     reply: {err}")
        seen.append(hit)
        time.sleep(0.5)

good = [s for s in seen if s and s[0] == EXPECT_LEN and (s[1], s[2]) == EXPECT_WH]
print(f"\n{len(good)}/{COUNT} frames match the ADR's prediction")

if not good:
    sys.exit(1)

fmts = sorted({s[3] for s in seen if s})
print(f"format value reported by the driver: {fmts}")
print("(4 is PIXFORMAT_JPEG, which is what this node used to return; anything "
      "else is what PIXFORMAT_GRAYSCALE enumerates to in this component build "
      "-- record it, do not assume it)")
sys.exit(0 if len(good) == COUNT else 2)
