#!/usr/bin/env python3
"""bench_die_rule.py — walkthrough §A5f: the first slot-bound rule on a real signal.

The XIAO's on-die temperature (`sensor.die_temperature`, no wiring) drives its
onboard LED (GPIO21, active-low) through two reflex rules bound to slot 0:

    die-hot   sensor.die_temperature >  T(slot 0)  →  gpio_write 21 = 0  (LED on)
    die-cool  sensor.die_temperature <= T(slot 0)  →  gpio_write 21 = 1  (LED off)

where T = 30 + level·40 °C, default level 0.5 (50 °C). Nothing here is a
synthetic snapshot: the node ticks its own sensor once a second. The script
reads the real die temperature, then slides the slot *around it* with
`descend` — threshold above the reading (LED must go off), below it (LED must
come on), above again — and verifies each transition with `gpio_read 21` and
the node's own `reflex` report lines. With `--base COM3` the descends go over
the authenticated LoRa link through the base station's console; without it
they go over USB.

    python scripts/bench_die_rule.py --node COM6 [--base COM3]

Records everything to results/bench_die_rule-<stamp>.json. A miss is data.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
import time

import serial

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from bench_run import Node  # noqa: E402
from bench_descend_lora import drain  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent
NODE_ID = "obc-esp32-s3-001"
LED_PIN = 21
SLOT = 0
T_MIN, T_MAX = 30.0, 70.0
DEBOUNCE_MS = 10_000


def rule(rid: str, op: str, value: int) -> dict:
    return {
        "id": rid,
        "when": {
            "type": "sensor_slot",
            "entity": "sensor.die_temperature",
            "op": op,
            "slot": SLOT,
            "min": T_MIN,
            "max": T_MAX,
            "default": 0.5,
        },
        "then": {"type": "gpio_write", "node_id": "self", "pin": LED_PIN, "value": value},
        "debounce_ms": DEBOUNCE_MS,
    }


RULES = [rule("die-hot", "gt", 0), rule("die-cool", "le", 1)]
LIMITS = [{"node_id": NODE_ID, "tool": "gpio_write", "allowed_pins": [LED_PIN],
           "value_min": 0, "value_max": 1, "min_interval_ms": 500}]


def level_for(threshold_c: float) -> float:
    return round(max(0.0, min(1.0, (threshold_c - T_MIN) / (T_MAX - T_MIN))), 3)


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--node", default="COM6")
    ap.add_argument("--base", default=None, help="base station console; descends go over LoRa")
    ap.add_argument("--settle", type=float, default=DEBOUNCE_MS / 1000 + 4,
                    help="seconds to wait for a rule to act after a slot move")
    args = ap.parse_args()

    node = Node(args.node, dry=False)
    base = None
    if args.base:
        base = serial.Serial()
        base.port, base.baudrate, base.timeout = args.base, 115200, 0.05
        base.dtr = base.rts = False
        base.open()
        base.dtr = base.rts = False
        time.sleep(0.3)
        base.reset_input_buffer()

    record: list[dict] = []
    misses: list[str] = []

    def step(name, cmd, cmd_args, expect, check, via="usb"):
        if via == "lora":
            # Reply-awaited retry, as mesh_command does: the mesh has no ACK and
            # a collision loses a frame now and then; descend is idempotent, and
            # each attempt carries its own id so a late reply cannot be
            # mistaken for the current one.
            reply = {}
            for attempt in range(3):
                rid = name if attempt == 0 else f"{name}r{attempt}"
                line = json.dumps({"id": rid, "to": NODE_ID, "cmd": cmd, "args": cmd_args}, separators=(",", ":"))
                base.reset_input_buffer()
                node.ser.reset_input_buffer()
                node.ser.timeout = 0.05
                base.write((line + "\n").encode())
                node_lines: list[str] = []
                _lines, got = drain(base, 10.0, want_id=rid, also=node.ser, also_lines=node_lines)
                node.ser.timeout = 2
                node.unsolicited.extend(node_lines)
                if got:
                    got["_attempts"] = attempt + 1
                    reply = got
                    break
                print(f"        no reply to {rid} within 10 s; retrying")
        else:
            reply = node.send(cmd, cmd_args, rid=name)
        ok = False
        try:
            ok = bool(check(reply))
        except Exception as e:  # noqa: BLE001
            reply = {"_check_error": str(e), **reply}
        print(f"[{'PASS' if ok else 'MISS'}] {via:4} {name}: {expect}")
        print(f"        got: {json.dumps(reply)[:220]}")
        record.append({"step": name, "via": via, "cmd": cmd, "args": cmd_args, "expect": expect,
                       "ok": ok, "reply": reply})
        if not ok:
            misses.append(name)
        return reply

    def led_level() -> str | None:
        r = node.send("gpio_read", {"pin": LED_PIN}, rid="led")
        return str(r.get("result")) if r.get("ok") else None

    def reports_since(mark: int) -> list[dict]:
        out = []
        for raw in node.unsolicited[mark:]:
            try:
                obj = json.loads(raw)
            except json.JSONDecodeError:
                continue
            if isinstance(obj, dict) and obj.get("type") == "reflex":
                out.append(obj)
        return out

    def wait_and_check(name, want_led: str, want_rule: str):
        mark = len(node.unsolicited)
        t0 = time.time()
        led, seen = None, []
        while time.time() - t0 < args.settle:
            # Keep the USB side drained so the node's reports are collected and
            # its USB-JTAG never blocks on us.
            node.ser.timeout = 0.05
            for _ in range(20):
                chunk = node.ser.readline().decode(errors="replace").strip()
                if chunk.startswith("{"):
                    node.unsolicited.append(chunk)
            node.ser.timeout = 2
            seen = [r for r in reports_since(mark) if r.get("rule_id") == want_rule]
            led = led_level()
            if led == want_led and seen:
                break
            time.sleep(1.0)
        ok = led == want_led and bool(seen) and all(r.get("applied") is True for r in seen)
        detail = f"LED pin {LED_PIN} reads {led} (want {want_led}); {len(seen)} '{want_rule}' report(s)" + \
                 (f", applied={[r.get('applied') for r in seen]}" if seen else "")
        print(f"[{'PASS' if ok else 'MISS'}] wait {name}: {detail}  ({time.time()-t0:.0f}s)")
        record.append({"step": name, "via": "observe", "ok": ok, "led": led, "reports": seen,
                       "seconds": round(time.time() - t0, 1)})
        if not ok:
            misses.append(name)

    # ── 1. The signal is real ──────────────────────────────────────────────
    r = step("read-die-temperature", "sensor_read", {"sensor": "esp32", "field": "die_temperature"},
             "a number, plausibly 20–70 °C",
             lambda r: r.get("ok") is True and 15.0 < float(r["result"]) < 85.0)
    if not r.get("ok"):
        print("no die temperature; nothing below is meaningful")
        return 1
    t_now = float(r["result"])
    above, below = level_for(t_now + 6.0), level_for(t_now - 6.0)
    print(f"        die temperature {t_now:.1f} °C → thresholds {t_now+6:.0f} °C (level {above}) "
          f"and {t_now-6:.0f} °C (level {below})")

    # ── 2. Gate + rules ────────────────────────────────────────────────────
    step("allow-led-pin", "set_limits", {"limits": LIMITS}, "applied, pin 21 allowed",
         lambda r: r.get("ok") is True and r["result"].get("applied") is True)
    step("push-die-rules", "set_reflex_rules", {"rules": RULES}, "loaded 2",
         lambda r: r.get("ok") is True and r["result"].get("loaded") == 2)
    via = "lora" if base else "usb"

    # ── 3. Threshold above the reading: the LED must go (or stay) off ──────
    step("slot-above", "descend", {"clear": True, "m": [[SLOT, above]]}, f"active [[0,{above}]]",
         lambda r: r.get("ok") is True and r["result"].get("active") == [[SLOT, above]], via=via)
    wait_and_check("led-off-when-cool", "1", "die-cool")

    # ── 4. Threshold below the reading: the LED must come on ───────────────
    step("slot-below", "descend", {"m": [[SLOT, below]]}, f"active [[0,{below}]]",
         lambda r: r.get("ok") is True and r["result"].get("active") == [[SLOT, below]], via=via)
    wait_and_check("led-on-when-hot", "0", "die-hot")

    # ── 5. And back above: off again ───────────────────────────────────────
    step("slot-above-again", "descend", {"m": [[SLOT, above]]}, f"active [[0,{above}]]",
         lambda r: r.get("ok") is True and r["result"].get("active") == [[SLOT, above]], via=via)
    wait_and_check("led-off-again", "1", "die-cool")

    # ── 6. Clear: back to the default 50 °C, whatever that means right now ─
    step("clear", "descend", {"clear": True}, "active []",
         lambda r: r.get("ok") is True and r["result"].get("active") == [], via=via)
    r = step("read-die-temperature-after", "sensor_read", {"sensor": "esp32", "field": "die_temperature"},
             "still a number", lambda r: r.get("ok") is True and 15.0 < float(r["result"]) < 85.0)

    stamp = time.strftime("%Y%m%d-%H%M%S")
    out = ROOT / "results" / f"bench_die_rule-{stamp}.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps({"node": args.node, "base": args.base, "stamp": stamp,
                               "die_temperature_c": t_now, "levels": {"above": above, "below": below},
                               "misses": misses, "steps": record,
                               "unsolicited": node.unsolicited[-200:]}, indent=2))
    print(f"\n{len(record) - len(misses)}/{len(record)} steps as stated; record -> {out}")
    if misses:
        print("misses: " + ", ".join(misses))
    return 1 if misses else 0


if __name__ == "__main__":
    sys.exit(main())
