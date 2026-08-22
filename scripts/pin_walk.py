#!/usr/bin/env python3
"""Drive one output pin at a time, so a person can see which pad moves.

Written 2026-08-22, when the control LED would not light in step 1-pre and
there was no way to tell from the bench whether the cause was the LED, the
breadboard row, the jumper, or the assumption that the pad marked `D2` is
GPIO 3. Those four have the same symptom and the bench procedure could not
separate them.

This does not test the safety gate and is not part of the record. It exists to
answer one question -- which physical pad moves when the firmware drives a
given GPIO -- and it answers it by driving the pins and letting a person look.

    python scripts/pin_walk.py                # cycle 3, 7, 8, 21 in turn
    python scripts/pin_walk.py --pin 3 --hold # hold GPIO 3 high until Enter
    python scripts/pin_walk.py --off          # drive every output pin low
    python scripts/pin_walk.py --port COM6

Only the pins in the node's boot allow-list can be driven: OUTPUT_PINS is
[21, 3, 7, 8], and a write to anything else is refused by the node itself.
That refusal is printed, not hidden -- a refused write and an unlit LED are
different findings and this script exists because they look the same.

GPIO 21 is in that list but is not brought out to the XIAO's 14-pin header, so
nothing on the breadboard will respond to it. It is included because the node
should still accept the write: if 21 is refused, the problem is the gate, not
the wiring.
"""

import argparse
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bench_run import Node  # noqa: E402

BOOT_PINS = [3, 7, 8, 21]


def write(node: Node, pin: int, value: int) -> bool:
    r = node.send("gpio_write", {"pin": pin, "value": value}, rid=f"walk-{pin}-{value}")
    ok = r.get("ok") is True
    if not ok:
        why = r.get("error") or r.get("_unparsed") or "(no reply)"
        kind = "REFUSED by the node" if r.get("refused") else "not ok"
        print(f"      {kind}: {why}")
    return ok


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--port")
    ap.add_argument("--pin", type=int, help="drive only this pin")
    ap.add_argument("--hold", action="store_true",
                    help="hold the pin high until you press Enter")
    ap.add_argument("--off", action="store_true",
                    help="drive every output pin low and exit")
    ap.add_argument("--seconds", type=float, default=2.0,
                    help="how long each pin stays high (default 2)")
    ap.add_argument("--passes", type=int, default=3)
    a = ap.parse_args()

    node = Node(a.port, False)
    pins = [a.pin] if a.pin else BOOT_PINS

    if a.off:
        for p in BOOT_PINS:
            print(f"  pin {p} -> 0")
            write(node, p, 0)
        return 0

    if a.hold:
        pin = a.pin or 3
        print(f"\n  Holding GPIO {pin} HIGH. It should read about 3.3 V against GND.")
        print("  Move your meter probe or your LED around the header while it is")
        print("  held -- whichever pad is at 3.3 V is GPIO %d, whatever the silk" % pin)
        print("  next to it says.\n")
        if not write(node, pin, 1):
            print("  The write did not succeed, so the pin is not high. Nothing")
            print("  you measure now says anything about the wiring.")
            return 1
        try:
            input("  Press Enter to drive it low again. ")
        finally:
            write(node, pin, 0)
            print(f"  GPIO {pin} -> 0")
        return 0

    print(f"\n  Walking {pins}, {a.seconds:g}s each, {a.passes} passes.")
    print("  Watch the board. Say which pad lights, and when.\n")
    for n in range(1, a.passes + 1):
        print(f"  pass {n}")
        for p in pins:
            note = "  (not on the XIAO header -- nothing should light)" if p == 21 else ""
            print(f"    GPIO {p} HIGH{note}")
            write(node, p, 1)
            time.sleep(a.seconds)
            write(node, p, 0)
            time.sleep(0.6)   # clear of the 500 ms rate limit, if one is loaded
    print("\n  Done. Every pin driven low.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
