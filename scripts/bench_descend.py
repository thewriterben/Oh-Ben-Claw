#!/usr/bin/env python3
"""bench_descend.py — walkthrough §A5b (the spinal tier) as a script.

Sends the eight A5b steps from docs/HARDWARE-TEST-WALKTHROUGH.md to the node
over its native USB serial, matches every reply by id (via bench_run.Node,
which learned the hard way not to take the first JSON line as the answer),
and writes what came back to results/bench_descend-<stamp>.json. It reports
each step against its stated expectation and does not stop on a miss — a miss
is data.

Step (h), the reboot, needs a hand on the board. The script asks.

    python scripts/bench_descend.py               # auto-detect the node's port
    python scripts/bench_descend.py --port COM6

Every tick is a synthetic snapshot; nothing here needs a sensor wired.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from bench_run import Node  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent

SLOT_RULE = {
    "id": "overheat",
    "when": {
        "type": "sensor_slot",
        "entity": "sensor.temperature",
        "op": "gt",
        "slot": 3,
        "min": 40.0,
        "max": 80.0,
        "default": 0.5,
    },
    "then": {"type": "gpio_write", "node_id": "self", "pin": 3, "value": 0},
    "debounce_ms": 1000,
}
BAD_SLOT_RULE = {
    "id": "bad",
    "when": {
        "type": "sensor_slot",
        "entity": "sensor.temperature",
        "op": "gt",
        "slot": 16,
        "min": 0.0,
        "max": 1.0,
        "default": 0.5,
    },
    "then": {"type": "escalate", "reason": "x"},
    "debounce_ms": 0,
}


def fired_ids(reply: dict) -> list[str]:
    res = reply.get("result")
    if isinstance(res, dict):
        return [f.get("rule_id") for f in res.get("fired", [])]
    return []


def active(reply: dict):
    res = reply.get("result")
    return res.get("active") if isinstance(res, dict) else None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    node = Node(args.port, args.dry_run)
    record: list[dict] = []
    misses: list[str] = []

    def step(name: str, cmd: str, cmd_args: dict, expect: str, check) -> dict:
        reply = node.send(cmd, cmd_args, rid=name)
        ok = False
        try:
            ok = bool(check(reply))
        except Exception as e:  # noqa: BLE001 — a malformed reply is a miss, not a crash
            reply = {"_check_error": str(e), **reply}
        mark = "PASS" if ok else "MISS"
        print(f"[{mark}] {name}: expected {expect}")
        print(f"        got: {json.dumps(reply)[:300]}")
        record.append({"step": name, "cmd": cmd, "args": cmd_args, "expect": expect, "ok": ok, "reply": reply})
        if not ok:
            misses.append(name)
        return reply

    tick = lambda t: {"snapshot": {"sensor.temperature": t}}  # noqa: E731

    # Start from a known rule set: the node merges pushed rules behind its
    # built-ins, so `capabilities` first just proves the port is the node.
    step("a0-capabilities", "capabilities", {}, "ok:true, a node id",
         lambda r: r.get("ok") is True)
    step("a-push-slot-rule", "set_reflex_rules", {"rules": [SLOT_RULE]},
         "ok:true, loaded 1",
         lambda r: r.get("ok") is True and r["result"].get("loaded") == 1)
    step("b-default-60-fires-at-65", "reflex_tick", tick(65.0),
         "'overheat' fires (and safe-overtemp-warn)",
         lambda r: "overheat" in fired_ids(r) and "safe-overtemp-warn" in fired_ids(r))
    step("c1-descend-1.0", "descend", {"m": [[3, 1.0]]},
         "ok:true, active [[3,1.0]]",
         lambda r: r.get("ok") is True and active(r) == [[3, 1.0]])
    step("c2-65-no-longer-fires", "reflex_tick", tick(65.0),
         "safe-overtemp-warn only, NO 'overheat'",
         lambda r: "overheat" not in fired_ids(r) and "safe-overtemp-warn" in fired_ids(r))
    step("d1-descend-0.0", "descend", {"m": [[3, 0.0]]},
         "ok:true, active [[3,0.0]]",
         lambda r: r.get("ok") is True and active(r) == [[3, 0.0]])
    step("d2-45-fires", "reflex_tick", tick(45.0),
         "'overheat' only",
         lambda r: fired_ids(r) == ["overheat"])
    step("e1-bad-slot-refused", "descend", {"m": [[3, 0.5], [16, 0.5]]},
         "ok:false, 'slot 16 out of range'",
         lambda r: r.get("ok") is False and "slot 16" in str(r.get("error")))
    step("e2-bad-level-refused", "descend", {"m": [[3, 1.5]]},
         "ok:false, 'not in [0, 1]'",
         lambda r: r.get("ok") is False and "not in [0, 1]" in str(r.get("error")))
    step("e3-active-unchanged", "descend", {"m": []},
         "ok:true, active still [[3,0.0]]",
         lambda r: r.get("ok") is True and active(r) == [[3, 0.0]])
    step("f1-clear", "descend", {"clear": True},
         "ok:true, active []",
         lambda r: r.get("ok") is True and active(r) == [])
    step("f2-65-fires-again", "reflex_tick", tick(65.0),
         "'overheat' fires again",
         lambda r: "overheat" in fired_ids(r))
    step("g-bad-rule-refused-at-door", "set_reflex_rules", {"rules": [BAD_SLOT_RULE]},
         "ok:false, 'rule bad: slot 16 out of range'",
         lambda r: r.get("ok") is False and "rule bad" in str(r.get("error")) and "slot 16" in str(r.get("error")))
    step("g2-rule-set-intact", "reflex_tick", tick(65.0),
         "'overheat' still fires (the refused push changed nothing)",
         lambda r: "overheat" in fired_ids(r))

    # (h) needs a hand on the board.
    step("h0-descend-0.0-before-reboot", "descend", {"m": [[3, 0.0]]},
         "ok:true, active [[3,0.0]]",
         lambda r: r.get("ok") is True and active(r) == [[3, 0.0]])
    if not args.dry_run:
        # The XIAO's native USB-Serial-JTAG re-enumerates on reset, so an open
        # handle dies with "WriteFile failed" the moment the board comes back.
        # Close before the reset, reopen after — with retries, because the
        # port takes a few seconds to reappear. Learned on the first run.
        node.ser.close()
        input("\n(h) Press the board's RESET button now, wait for it to come back "
              "(~5 s), then press Enter here... ")
        import serial  # noqa: PLC0415
        for attempt in range(20):
            try:
                s = serial.Serial()
                s.port, s.baudrate, s.timeout = node.port, 115200, 2
                s.dtr = s.rts = False  # a default open can reset the S3 again
                s.open()
                s.dtr = s.rts = False
                node.ser = s
                break
            except serial.SerialException:
                time.sleep(0.5)
        else:
            sys.exit(f"{node.port} did not come back after the reset")
        time.sleep(1.0)
        node.ser.reset_input_buffer()
        print(f"        reopened {node.port} after {attempt * 0.5:.1f} s")
    # A reboot also drops the pushed rule (rules are RAM too), so the check is
    # on the level, and the rule has to be pushed again first.
    step("h1-repush-after-reboot", "set_reflex_rules", {"rules": [SLOT_RULE]},
         "ok:true, loaded 1",
         lambda r: r.get("ok") is True and r["result"].get("loaded") == 1)
    step("h2-level-forgotten-45-does-not-fire", "reflex_tick", tick(45.0),
         "nothing fires: level is RAM-only, threshold back at 60",
         lambda r: "overheat" not in fired_ids(r))
    step("h3-65-fires-at-default", "reflex_tick", tick(65.0),
         "'overheat' fires at the default",
         lambda r: "overheat" in fired_ids(r))

    stamp = time.strftime("%Y%m%d-%H%M%S")
    out = ROOT / "results" / f"bench_descend-{stamp}.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps({
        "port": node.port,
        "stamp": stamp,
        "misses": misses,
        "steps": record,
        "unsolicited": node.unsolicited[-40:],
    }, indent=2))
    print(f"\n{len(record) - len(misses)}/{len(record)} steps as stated; record -> {out}")
    if misses:
        print("misses: " + ", ".join(misses))
    return 1 if misses else 0


if __name__ == "__main__":
    sys.exit(main())
