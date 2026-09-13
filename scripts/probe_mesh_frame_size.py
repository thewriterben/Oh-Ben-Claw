#!/usr/bin/env python3
"""probe_mesh_frame_size.py — does a command of N bytes cross the mesh?

Sends `gpio_read` commands of increasing size (padded with a `pad` arg the
node ignores) from the base console and reports, per size, whether the base
transmitted it, whether the bridge heard it, and whether the node answered.
Both consoles are opened quietly. The brain must NOT hold the base.

    python scripts/probe_mesh_frame_size.py --base COM3 --bridge COM7 --sizes 90,150,180,205,220,228

Found for on 2026-09-13: seven 205-byte set_limits pushes in a row never
reached the bridge while 117-byte beacons flowed the other way.
"""

from __future__ import annotations

import argparse
import json
import sys
import time

import serial

sys.path.insert(0, __import__("pathlib").Path(__file__).resolve().parent.as_posix())
from bench_die_rule import NODE_ID  # noqa: E402


def quiet(port: str) -> serial.Serial:
    s = serial.Serial()
    s.port, s.baudrate, s.timeout = port, 115200, 0.05
    s.dtr = s.rts = False
    s.open()
    s.dtr = s.rts = False
    time.sleep(0.3)
    s.reset_input_buffer()
    return s


def drain(s: serial.Serial, seconds: float) -> list[str]:
    out, buf, t0 = [], b"", time.time()
    while time.time() - t0 < seconds:
        buf += s.read(4096)
        *whole, buf = buf.split(b"\n")
        out.extend(w.decode(errors="replace").rstrip() for w in whole)
    return out


def command_of(size: int, i: int) -> str:
    rid = f"sz{size}{i}"
    base = {"id": rid, "to": NODE_ID, "cmd": "gpio_read", "args": {"pin": 21}}
    line = json.dumps(base, separators=(",", ":"))
    pad = size - len(line) - len(',"pad":""')
    if pad > 0:
        base["args"]["pad"] = "x" * pad
        line = json.dumps(base, separators=(",", ":"))
    return rid, line


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", default="COM3")
    ap.add_argument("--bridge", default="COM7")
    ap.add_argument("--sizes", default="90,150,180,205,220,228")
    ap.add_argument("--wait", type=float, default=8.0)
    args = ap.parse_args()

    base, bridge = quiet(args.base), quiet(args.bridge)
    results = []
    for i, size in enumerate(int(x) for x in args.sizes.split(",")):
        rid, line = command_of(size, i)
        base.reset_input_buffer()
        bridge.reset_input_buffer()
        base.write((line + "\n").encode())
        t0 = time.time()
        b_lines, g_lines = [], []
        while time.time() - t0 < args.wait:
            b_lines += drain(base, 0.2)
            g_lines += drain(bridge, 0.2)
        tx = any("(console)" in l and rid in l for l in b_lines)
        heard = any("SPINE ◄" in l and rid in l for l in g_lines)
        rejected = any("REJECTED" in l for l in g_lines)
        reply = any("SPINE ◄" in l and rid in l and '"ok"' in l for l in b_lines)
        results.append((len(line), tx, heard, rejected, reply))
        print(f"{len(line):4d} B  base tx={'y' if tx else 'N'}  bridge heard={'y' if heard else 'N'}"
              f"{' (REJECTED)' if rejected else ''}  node replied={'y' if reply else 'N'}")
    base.close()
    bridge.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
