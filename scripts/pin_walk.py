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

A lit pad is not evidence. Some pads are high before this script runs: GPIO 43
(silk `D6`) is the UART1 spine uplink and an idle UART transmit line sits HIGH
from boot, so an LED there glows steadily no matter which pin you drive. On
2026-08-22 that produced a false positive -- `D6` lit while GPIO 3 was held
high, which reads as "D6 is GPIO 3" and is not. Prefer `--blink`: a pad that
changes in time with the script is the pin you drove; a pad that is merely lit
is a pad that was already lit.
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
    ap.add_argument("--blink", action="store_true",
                    help="blink the pin ~1 Hz forever. Prefer this to --hold: a "
                         "pad that is lit may simply be an idle UART line, and "
                         "only a pad that blinks in time is the pin you drove.")
    ap.add_argument("--off", action="store_true",
                    help="drive every output pin low and exit")
    ap.add_argument("--report", action="store_true",
                    help="write each boot-policy pin high, read it back, write it "
                         "low. Says whether the pin follows the write, without "
                         "needing anybody to look at an LED.")
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

    if a.report:
        print("\n  Writing each boot-policy pin high, reading it back, writing it low.")
        print("  The read-back only means something because the firmware now")
        print("  configures these pins INPUT_OUTPUT: with plain OUTPUT the input")
        print("  path is off and gpio_get_level returns 0 whatever the pin does.")
        print()
        for pin in BOOT_PINS:
            before = node.send("gpio_read", {"pin": pin}, rid=f"rep-{pin}-b").get("result")
            w1 = node.send("gpio_write", {"pin": pin, "value": 1}, rid=f"rep-{pin}-1")
            time.sleep(0.15)
            high = node.send("gpio_read", {"pin": pin}, rid=f"rep-{pin}-h").get("result")
            node.send("gpio_write", {"pin": pin, "value": 0}, rid=f"rep-{pin}-0")
            time.sleep(0.15)
            low = node.send("gpio_read", {"pin": pin}, rid=f"rep-{pin}-l").get("result")
            verdict = ("pin follows the write" if (high == 1 and low == 0)
                       else "WRITE ok, PIN DID NOT MOVE" if w1.get("ok")
                       else "write refused")
            print(f"    GPIO {pin:>2}: before={before!r} after write 1={high!r} "
                  f"after write 0={low!r}   {verdict}")
            time.sleep(0.7)

        print("\n  Pins this build drives for its own reasons, for comparison:")
        for pin, what in ((43, "UART1 TX, silk D6 -- idles HIGH"),
                          (1, "I2S WS, silk D0"),
                          (0, "I2S SCK"),
                          (2, "I2S SD")):
            r = node.send("gpio_read", {"pin": pin}, rid=f"cmp-{pin}").get("result")
            print(f"    GPIO {pin:>2}: {r!r}   ({what})")

        if node.unsolicited:
            print(f"\n  {len(node.unsolicited)} unsolicited line(s) from the node were")
            print("  skipped while matching replies by id. That is the node's own")
            print("  telemetry, not an answer to anything asked here.")
        return 0

    if a.blink:
        pin = a.pin or 3
        print(f"\n  Blinking GPIO {pin} about once a second, forever. Ctrl+C to stop.")
        print("  Look for a pad that BLINKS, not one that is lit.")
        print()
        print("  A steadily lit pad proves nothing: GPIO 43 (silk `D6`) is the")
        print("  UART1 spine uplink, and an idle UART transmit line sits HIGH.")
        print("  It is lit from boot, whatever this script does. So is anything")
        print("  else with an internal pull-up. Only the blink is evidence.")
        print()
        try:
            while True:
                write(node, pin, 1)
                time.sleep(0.6)
                write(node, pin, 0)
                time.sleep(0.6)
        except KeyboardInterrupt:
            write(node, pin, 0)
            print(f"\n  GPIO {pin} -> 0")
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
