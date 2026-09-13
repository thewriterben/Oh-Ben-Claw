#!/usr/bin/env python3
"""probe_mesh_listen.py — print what the base station hears for --seconds, one line per frame.

    python scripts/probe_mesh_listen.py --base COM3 --seconds 30 [--grep reflex]
"""

import argparse
import pathlib
import sys
import time

import serial

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from bench_descend_lora import drain  # noqa: E402


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--base", default="COM3")
    ap.add_argument("--seconds", type=float, default=30.0)
    ap.add_argument("--grep", default="SPINE ◄")
    args = ap.parse_args()
    s = serial.Serial()
    s.port, s.baudrate, s.timeout = args.base, 115200, 0.05
    s.dtr = s.rts = False
    s.open()
    s.dtr = s.rts = False
    time.sleep(0.3)
    s.reset_input_buffer()
    lines, _ = drain(s, args.seconds)
    for l in lines:
        if args.grep in l:
            i = l.find(" : ")
            print(l[:8] + (l[i + 3:][:170] if i > 0 else l[:170]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
