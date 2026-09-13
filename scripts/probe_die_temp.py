#!/usr/bin/env python3
"""probe_die_temp.py — is the die temperature a live signal or a number?

Reads `sensor_read esp32/die_temperature` every second for --seconds and
prints the values, min, max and how many distinct readings there were. Warm
the board (a finger on the module for ten seconds is enough) partway through
if you want to see it move.

    python scripts/probe_die_temp.py --port COM6 --seconds 30
"""

import argparse
import pathlib
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from bench_run import Node  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--port", default="COM6")
    ap.add_argument("--seconds", type=float, default=30.0)
    args = ap.parse_args()
    node = Node(args.port, dry=False)
    vals = []
    t0 = time.time()
    while time.time() - t0 < args.seconds:
        r = node.send("sensor_read", {"sensor": "esp32", "field": "die_temperature"}, rid="t")
        if r.get("ok"):
            v = float(r["result"])
            vals.append(v)
            print(f"{time.time()-t0:5.1f}s  {v:.1f} °C")
        else:
            print(f"{time.time()-t0:5.1f}s  {r}")
        time.sleep(1.0)
    if vals:
        print(f"min {min(vals):.1f}  max {max(vals):.1f}  distinct {len(set(vals))} of {len(vals)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
