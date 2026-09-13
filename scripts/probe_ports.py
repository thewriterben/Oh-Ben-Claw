#!/usr/bin/env python3
"""probe_ports.py — which port is which, after a bench reset.

Lists every serial port with its USB identity, then listens to each Heltec
console for a few seconds — quietly (DTR/RTS low, so nothing reboots) — and
says which station it is from what it hears: the base hears the bridge
(`src=40`), the bridge hears the base (`src=D8`). The XIAO is the one native
Espressif port (VID 0x303A); it is not opened here.

    python scripts/probe_ports.py [--seconds 8]

Nothing is written to any port.
"""

from __future__ import annotations

import argparse
import re
import sys
import time

import serial
from serial.tools import list_ports


def listen(port: str, seconds: float) -> tuple[set[str], int]:
    s = serial.Serial()
    s.port, s.baudrate, s.timeout = port, 115200, 0.05
    s.dtr = s.rts = False
    s.open()
    s.dtr = s.rts = False
    srcs: set[str] = set()
    lines = 0
    t0 = time.time()
    buf = b""
    while time.time() - t0 < seconds:
        buf += s.read(4096)
        *whole, buf = buf.split(b"\n")
        for raw in whole:
            lines += 1
            m = re.search(rb"src=([0-9A-F]{2})", raw)
            if m:
                srcs.add(m.group(1).decode())
    s.close()
    return srcs, lines


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--seconds", type=float, default=8.0)
    args = ap.parse_args()

    ports = list(list_ports.comports())
    for p in ports:
        print(f"{p.device}: vid={p.vid:#06x} pid={p.pid:#06x} {p.description} sn={p.serial_number}")
    print()
    for p in ports:
        if p.vid == 0x303A:
            print(f"{p.device}: the XIAO (native USB-Serial-JTAG) — not opened")
        elif p.vid in (0x10C4, 0x1A86, 0x0403, 0x067B):
            try:
                srcs, n = listen(p.device, args.seconds)
            except serial.SerialException as e:
                # A port something else holds is a fact worth naming: the
                # running brain's `[lora_gateway] port` is the usual holder.
                print(f"{p.device}: busy — {e.args[0].split(':')[-1].strip()} (the brain's lora_gateway?)")
                continue
            role = ("BASE gw-D8 (hears the bridge)" if "40" in srcs and "D8" not in srcs
                    else "BRIDGE gw-40 (hears the base)" if "D8" in srcs and "40" not in srcs
                    else f"unclear — heard src={sorted(srcs)}")
            print(f"{p.device}: {n} lines in {args.seconds:.0f}s, src seen {sorted(srcs)} → {role}")
        else:
            print(f"{p.device}: unknown kind")
    return 0


if __name__ == "__main__":
    sys.exit(main())
