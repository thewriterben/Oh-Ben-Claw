#!/usr/bin/env python3
"""bench_chatter.py — how much a holding reflex costs the mesh, and what that costs commands.

Pushes the §A5f die-temperature rules over USB (so the die-cool rule holds at
the default 50 °C), then for --seconds counts every frame the base station
hears by kind, then sends --probes `gpio_read` commands over the mesh and
counts the replies. Run once on a node without edge-triggering and once with;
the difference is what `fire_on_change` buys.

    python scripts/bench_chatter.py --node COM6 --base COM3 [--seconds 90] [--probes 6]

Records to results/bench_chatter-<stamp>.json.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
import time

import serial

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from bench_run import Node  # noqa: E402
from bench_die_rule import LIMITS, RULES, NODE_ID  # noqa: E402
from bench_descend_lora import drain  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--node", default="COM6")
    ap.add_argument("--base", default="COM3")
    ap.add_argument("--seconds", type=float, default=90.0)
    ap.add_argument("--probes", type=int, default=6)
    ap.add_argument("--label", default="")
    args = ap.parse_args()

    node = Node(args.node, dry=False)
    r = node.send("set_limits", {"limits": LIMITS}, rid="lim")
    assert r.get("ok"), r
    r = node.send("set_reflex_rules", {"rules": RULES}, rid="rules")
    assert r.get("ok") and r["result"].get("loaded") == 2, r
    fw = node.send("capabilities", {}, rid="cap").get("result", {})
    version = fw.get("firmware_version") if isinstance(fw, dict) else None
    print(f"rules loaded on {NODE_ID} (firmware {version}); closing the node's USB so it runs unattended")
    node.ser.close()
    time.sleep(2.0)

    base = serial.Serial()
    base.port, base.baudrate, base.timeout = args.base, 115200, 0.05
    base.dtr = base.rts = False
    base.open()
    base.dtr = base.rts = False
    time.sleep(0.3)
    base.reset_input_buffer()

    print(f"listening {args.seconds:.0f}s ...")
    lines, _ = drain(base, args.seconds)
    kinds: dict[str, int] = {}
    for l in lines:
        if "SPINE ◄" not in l:
            continue
        # `drain` keeps 260 chars of a line, which on a reflex report ends
        # before `rule_id`; classify by the payload's first `type`, which for
        # a report is the action's (`gpio_write` = a die rule, `escalate` =
        # safing), and by `rule_id` when it did fit.
        m = re.search(r'"rule_id":"([^"]+)"', l)
        if m:
            k = "reflex:" + m.group(1)
        else:
            m = re.search(r'"type":"(\w+)"', l)
            k = m.group(1) if m else "other"
            if k in ("gpio_write", "escalate"):
                k = "reflex-" + k
        kinds[k] = kinds.get(k, 0) + 1
    total = sum(kinds.values())
    per_min = {k: round(v * 60.0 / args.seconds, 1) for k, v in sorted(kinds.items())}
    print(f"frames heard: {total} in {args.seconds:.0f}s → per minute: {per_min}")

    replies = 0
    attempts = []
    for i in range(args.probes):
        rid = f"ch{int(time.time()) % 100000}{i}"
        line = json.dumps({"id": rid, "to": NODE_ID, "cmd": "gpio_read", "args": {"pin": 21}}, separators=(",", ":"))
        base.reset_input_buffer()
        base.write((line + "\n").encode())
        _, reply = drain(base, 8.0, want_id=rid)
        attempts.append(bool(reply))
        replies += 1 if reply else 0
        print(f"  probe {i+1}: {'reply' if reply else 'no reply'}")
    print(f"commands answered: {replies}/{args.probes}")

    stamp = time.strftime("%Y%m%d-%H%M%S")
    out = ROOT / "results" / f"bench_chatter-{stamp}.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps({"label": args.label, "stamp": stamp, "firmware": version,
                               "seconds": args.seconds, "frames": kinds, "per_minute": per_min,
                               "probes": attempts, "answered": replies,
                               "lines": [l[:200] for l in lines if "SPINE" in l]}, indent=2))
    print(f"record -> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
