#!/usr/bin/env python3
"""probe_usb_drain.py — hold the node's USB port open and read it, for --seconds.

Exists to test one hypothesis: that the node answers mesh commands only while
a host is reading its USB console (the XIAO's USB-Serial-JTAG stalls every
write when nothing reads). Run this in one window and a mesh command in
another; if the reply appears only while this runs, the stall is the cause.

    python scripts/probe_usb_drain.py --port COM6 --seconds 60
"""

import argparse
import sys
import time

import serial


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--port", default="COM6")
    ap.add_argument("--seconds", type=float, default=60.0)
    args = ap.parse_args()
    # DTR/RTS held low: the S3's USB-JTAG resets the chip on the wrong edge.
    s = serial.Serial()
    s.port, s.baudrate, s.timeout = args.port, 115200, 0.2
    s.dtr = s.rts = False
    s.open()
    s.dtr = s.rts = False
    t0 = time.time()
    n = 0
    while time.time() - t0 < args.seconds:
        line = s.readline().decode(errors="replace").strip()
        if line:
            n += 1
            print(f"{time.time()-t0:5.1f}s {line[:140]}")
    print(f"{n} lines")
    return 0


if __name__ == "__main__":
    sys.exit(main())
