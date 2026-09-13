#!/usr/bin/env python3
"""probe_mesh_cmd.py — send one command over the mesh from the base console and print what comes back.

    python scripts/probe_mesh_cmd.py --base COM3 --cmd gpio_read --args '{"pin":21}'

Opens the base without resetting it, writes the command line, and prints every
base console line for --wait seconds, marking the reply whose id matches.
Nothing else is opened: if the node's reply does not show here, the problem is
on the air or on the node, not in the host's serial thread.
"""

import argparse
import json
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
    ap.add_argument("--node", default="obc-esp32-s3-001")
    ap.add_argument("--cmd", default="gpio_read")
    ap.add_argument("--args", default='{"pin":21}')
    ap.add_argument("--wait", type=float, default=10.0)
    ap.add_argument("--bridge", default=None, help="also show the bridge station's console (e.g. COM5)")
    args = ap.parse_args()

    def quiet_open(port):
        s = serial.Serial()
        s.port, s.baudrate, s.timeout = port, 115200, 0.05
        s.dtr = s.rts = False
        s.open()
        s.dtr = s.rts = False
        return s

    base = quiet_open(args.base)
    bridge = quiet_open(args.bridge) if args.bridge else None
    time.sleep(0.3)
    base.reset_input_buffer()
    if bridge:
        bridge.reset_input_buffer()
    rid = f"pm{int(time.time()) % 100000}"
    line = json.dumps({"id": rid, "to": args.node, "cmd": args.cmd, "args": json.loads(args.args)},
                      separators=(",", ":"))
    print(f"→ {line}")
    base.write((line + "\n").encode())
    bridge_lines: list[str] = []
    lines, reply = drain(base, args.wait, want_id=rid, also=bridge, also_lines=bridge_lines)
    for l in lines:
        if "SPINE" in l:
            print("   base:   " + l[:200])
    for l in bridge_lines:
        if "SPINE" in l or "uart" in l:
            print("   bridge: " + l[:200])
    print(f"reply: {json.dumps(reply) if reply else 'NONE'}")
    return 0 if reply else 1


if __name__ == "__main__":
    sys.exit(main())
