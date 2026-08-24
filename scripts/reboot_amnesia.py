#!/usr/bin/env python3
"""Show that a reboot silently widens this node's safety policy.

Recorded 2026-08-22. The bench run found that a pushed limit table was not
enforced. The gate's logic was not the problem -- `cargo test --test
firmware_node_gates` replays the exact table and refuses correctly, including a
test written from the literal JSON the runner puts on the wire. The problem is
the lifecycle around it.

Two facts, both reproduced by this script:

1. The node reboots on its own. A JSON-heavy reply -- `capabilities` is 1170
   bytes -- overflows the main task stack. The reply arrives truncated at
   ~1088 bytes and is followed by
   `***ERROR*** A stack overflow in task main has been detected.` and a reset.
   Observed on roughly every other request in a five-request sweep.

2. A reboot discards the pushed policy and restores the boot allow-list, which
   is WIDER, and nothing tells the host. `set_limits` reports
   `allowed_pins [3,7]`; a moment and one crash later the node reports
   `[21,3,7,8]` with `min_interval_ms: null`, and accepts a write to pin 8.

The second fact is the serious one and does not depend on the first. The pushed
policy lives only in RAM. Any reset -- crash, watchdog, brown-out, someone
touching the USB cable -- reopens pins the host believes are locked, and the
host has no way to know. docs/SAFETY-CASE.md claims a compromised or absent host
cannot talk the node round. It does not have to: it only has to reset it.

Run it with the node attached and nothing else holding the port.
"""

import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bench_run import LIMITS, NODE_ID  # noqa: E402

import serial  # noqa: E402
from serial.tools import list_ports  # noqa: E402


class Link:
    """Deliberately not `bench_run.Node`: this script has to see the crash.

    `Node.send` skips anything that is not a JSON reply with a matching id,
    which is right for the runner and wrong here -- the panic banner and the ROM
    boot header are the evidence.
    """

    def __init__(self):
        native = [p for p in list_ports.comports() if (p.vid or 0) == 0x303A]
        if len(native) != 1:
            sys.exit("expected exactly one ESP32-S3 native USB port (VID 0x303a)")
        self.ser = serial.Serial(native[0].device, 115200, timeout=0.4)
        self.port = native[0].device
        time.sleep(0.3)
        self.ser.reset_input_buffer()
        self.seq = 0

    def send(self, cmd, args=None, wait=3.0):
        self.seq += 1
        rid = f"amnesia-{self.seq}"
        self.ser.write((json.dumps({"id": rid, "cmd": cmd, "args": args or {}}) + "\n").encode())
        end, rebooted = time.time() + wait, False
        while time.time() < end:
            raw = self.ser.readline().decode(errors="replace").strip()
            if not raw:
                continue
            if "stack overflow" in raw or raw.startswith("ESP-ROM"):
                rebooted = True
            if raw.startswith("{"):
                try:
                    obj = json.loads(raw)
                except json.JSONDecodeError:
                    continue          # a truncated reply is not a reply
                if obj.get("id") == rid:
                    return obj, rebooted
        return None, rebooted

    def policy(self):
        """Read the active policy without changing it.

        An empty `limits` list matches nothing, so `apply_pushed` returns false
        and leaves the gate alone -- but `set_limits` still reports the policy.
        """
        r, reboot = self.send("set_limits", {"limits": []})
        return (json.loads(r["result"]) if r else None), reboot


def main() -> int:
    link = Link()
    print(f"\n  node on {link.port}\n")

    r, _ = link.send("set_limits", {"limits": LIMITS})
    if not r:
        print("  the node did not answer the push. Try again.")
        return 2
    print(f"  pushed:   {r['result']}")

    pol, rebooted = link.policy()
    if pol is None:
        print("  the node did not report its policy back.")
        return 2
    print(f"  reads as: allowed_pins={pol.get('allowed_pins')} "
          f"min_interval_ms={pol.get('min_interval_ms')}")
    if rebooted:
        print("  (a reboot happened between the push and this read)")

    widened = pol.get("allowed_pins") != [3, 7]
    w, _ = link.send("gpio_write", {"pin": 8, "value": 1})
    link.send("gpio_write", {"pin": 8, "value": 0})
    accepted = w is not None and w.get("ok") is True

    print()
    if widened or accepted:
        print("  FAIL-OPEN CONFIRMED")
        print(f"    the pushed table was [3, 7]; the node now reports "
              f"{pol.get('allowed_pins')}")
        print(f"    a write to pin 8 was {'ACCEPTED' if accepted else 'refused'}")
        print("    the host was told the tighter policy applied and was not told")
        print("    when it stopped applying.")
        return 1

    print("  The policy survived and pin 8 was refused.")
    print(f"  Node {NODE_ID} is holding the table it was given.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
