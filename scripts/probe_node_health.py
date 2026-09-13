#!/usr/bin/env python3
"""probe_node_health.py — is the XIAO alive and doing what its firmware says?

Over USB only, so it runs while the brain holds the base station: identity
(`capabilities`), the real die temperature, the LED pin, the rules loaded,
and then a quiet listen for the node's own lines — beacon, link_state,
reflex reports (which carry `ev` since 2026-09-13). Nothing is pushed and
nothing actuated; the LED is read, not written.

    python scripts/probe_node_health.py [--port COM6] [--listen 20]
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from bench_run import Node  # noqa: E402


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", default="COM6")
    ap.add_argument("--listen", type=float, default=20.0)
    args = ap.parse_args()

    node = Node(args.port, dry=False)
    ok = True

    def check(name, cmd, cmd_args, test):
        nonlocal ok
        r = node.send(cmd, cmd_args, rid=name)
        good = False
        try:
            good = bool(test(r))
        except Exception:  # noqa: BLE001
            good = False
        ok &= good
        print(f"[{'PASS' if good else 'MISS'}] {name}: {json.dumps(r)[:200]}")
        return r

    cap = check("capabilities", "capabilities", {},
                lambda r: r.get("ok") and r["result"].get("edge_agent") is True)
    res = cap.get("result", {}) if isinstance(cap.get("result"), dict) else {}
    print(f"        node_id={res.get('node_id')} firmware={res.get('firmware_version')} board={res.get('board')}")
    check("die-temperature", "sensor_read", {"sensor": "esp32", "field": "die_temperature"},
          lambda r: r.get("ok") and 15.0 < float(r["result"]) < 85.0)
    check("led-pin-readable", "gpio_read", {"pin": 21}, lambda r: r.get("ok") and r["result"] in ("0", "1", 0, 1))
    check("reflex-tick-answers", "reflex_tick", {"snapshot": {"sensor.link_silence_ms": 0.0}},
          lambda r: r.get("ok") and isinstance(r["result"].get("fired"), list))

    print(f"listening {args.listen:.0f}s for the node's own lines (USB drained, nothing sent) ...")
    node.ser.timeout = 0.05
    kinds: dict[str, int] = {}
    samples: dict[str, str] = {}
    t0 = time.time()
    while time.time() - t0 < args.listen:
        raw = node.ser.readline().decode(errors="replace").strip()
        if not raw.startswith("{"):
            continue
        try:
            obj = json.loads(raw)
        except json.JSONDecodeError:
            continue
        k = obj.get("type", "?")
        if k == "reflex":
            k = f"reflex:{obj.get('rule_id')}"
        kinds[k] = kinds.get(k, 0) + 1
        samples.setdefault(k, raw[:160])
    node.ser.timeout = 2
    print(f"  heard: {kinds or 'nothing'}")
    for k, s in samples.items():
        print(f"    {k}: {s}")
    beacon_ok = kinds.get("beacon", 0) >= 1 if args.listen >= 31 else True
    ok &= beacon_ok
    print("PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
