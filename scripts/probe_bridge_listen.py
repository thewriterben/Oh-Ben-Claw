#!/usr/bin/env python3
"""probe_bridge_listen.py — what the bridge (gw-40) hears and forwards, verbatim.

Opens the bridge's console quietly (DTR/RTS low — a default open reboots it and
its de-dup ring drops the next commands) and prints every line for --seconds,
so a command the base transmitted can be seen arriving (`SPINE ◄ … : {json}`),
being refused (`SPINE ◄ REJECTED …`), or never showing up at all. Nothing is
sent.

    python scripts/probe_bridge_listen.py --port COM7 --seconds 60 [--grep lim]
"""

from __future__ import annotations

import argparse
import sys
import time

import serial


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", default="COM7")
    ap.add_argument("--seconds", type=float, default=60.0)
    ap.add_argument("--grep", default="", help="only print lines containing this")
    args = ap.parse_args()

    s = serial.Serial()
    s.port, s.baudrate, s.timeout = args.port, 115200, 0.05
    s.dtr = s.rts = False
    s.open()
    s.dtr = s.rts = False
    t0 = time.time()
    buf = b""
    n = 0
    while time.time() - t0 < args.seconds:
        buf += s.read(4096)
        *whole, buf = buf.split(b"\n")
        for raw in whole:
            line = raw.decode(errors="replace").rstrip()
            n += 1
            if args.grep and args.grep not in line:
                continue
            print(f"{time.time() - t0:6.1f} {line[:230]}")
    s.close()
    print(f"({n} lines in {args.seconds:.0f}s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
