"""Measure what gw-40 actually hears from gw-D8, and how much of it it relays.

    python relay_loss.py COM4 <seconds> <label>

The claim under test (firmware Cargo.toml, measured 2026-09-12): a base station
that flood-relays is deaf for the ~300 ms each relay takes, and that cost gw-40
"about half of everything gw-D8 sent — the frame after any received frame,
reliably."

The metric is delivery, not counts: gw-D8 stamps every frame with `seq=`, so the
frames gw-40 missed are the gaps in that sequence. Counting received frames
alone cannot see a miss; counting gaps can.

Also reported: how many relays gw-40 performed, and whether a miss follows a
relay — which is the specific mechanism the comment names.
"""

import re
import subprocess
import sys
import time

import serial

PORT = sys.argv[1] if len(sys.argv) > 1 else "COM4"
SECONDS = float(sys.argv[2]) if len(sys.argv) > 2 else 180.0
LABEL = sys.argv[3] if len(sys.argv) > 3 else "unlabelled"

RX = re.compile(r"SPINE\s+\S+\s+src=D8\s+seq=(\d+)")
RELAY = re.compile(r"SPINE\s+\S+\s+relay\s+src=D8\s+seq=(\d+)")

subprocess.run(["espflash", "reset", "--port", PORT], capture_output=True, timeout=60)
time.sleep(0.5)

events = []          # (kind, seq) in arrival order
buf = ""
with serial.Serial(PORT, 115200, timeout=0.3) as ser:
    end = time.time() + SECONDS
    while time.time() < end:
        c = ser.read(4096)
        if not c:
            continue
        buf += c.decode("utf-8", errors="replace")
        while "\n" in buf:
            line, buf = buf.split("\n", 1)
            m = RELAY.search(line)
            if m:
                events.append(("relay", int(m.group(1))))
                continue
            m = RX.search(line)
            if m:
                events.append(("rx", int(m.group(1))))

rx = [s for k, s in events if k == "rx"]
relays = [s for k, s in events if k == "relay"]

print(f"\n=== {LABEL} on {PORT}, {SECONDS:.0f}s ===")
if len(rx) < 2:
    print(f"  only {len(rx)} frame(s) from D8 — not enough to measure")
    sys.exit(1)

span = max(rx) - min(rx) + 1
got = len(set(rx))
missed = span - got
print(f"  D8 seq range   : {min(rx)}..{max(rx)}  (span {span})")
print(f"  received       : {got}")
print(f"  missed (gaps)  : {missed}")
print(f"  delivery       : {100.0*got/span:.1f}%")
print(f"  relays by gw-40: {len(relays)}")

# The specific mechanism: is the frame AFTER a relay the one that goes missing?
seen = sorted(set(rx))
gaps = []
for a, b in zip(seen, seen[1:]):
    if b - a > 1:
        gaps.extend(range(a + 1, b))
after_relay = sum(1 for g in gaps if (g - 1) in relays)
if gaps:
    print(f"  of {len(gaps)} missing seq, {after_relay} directly follow a relayed frame "
          f"({100.0*after_relay/len(gaps):.0f}%)")
else:
    print("  no gaps at all")
