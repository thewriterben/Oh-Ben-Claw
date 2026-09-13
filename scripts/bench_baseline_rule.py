#!/usr/bin/env python3
"""bench_baseline_rule.py — a rule that thresholds a reading against its own history.

Pushes one rule to the node over USB:

    die-rising  sensor.die_temperature > baseline(tau 60 s) + OFFSET  →  gpio_write 21 = 0 (LED on)

with `hold_ms` 3000 and `fire_on_change`, so it acts when the die has been
OFFSET °C above its own last-minute average for three ticks. A fixed
threshold cannot express this: the die sits wherever the room and the load
put it, and only a *departure* from that is the event.

Two phases, the first unattended:

  steady   watch for --steady seconds; the rule must not fire (the reading
           drifts, the baseline follows it, nothing is a departure);
  warm     with --warm: the operator is asked to warm the module (a thumb on
           the XIAO's metal can does it in ~20 s); the rule must fire once,
           `applied: true`, and not again while the die stays warm.

    python scripts/bench_baseline_rule.py --node COM6 [--warm]

Records to results/bench_baseline_rule-<stamp>.json.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from bench_run import Node  # noqa: E402
from bench_die_rule import LIMITS, NODE_ID, LED_PIN  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent
OFFSET_C = 2.0
TAU_S = 60.0
HOLD_MS = 3_000

RULE = {
    "id": "die-rising",
    "when": {
        "type": "sensor_baseline",
        "entity": "sensor.die_temperature",
        "op": "gt",
        "offset": OFFSET_C,
        "tau_s": TAU_S,
    },
    "then": {"type": "gpio_write", "node_id": "self", "pin": LED_PIN, "value": 0},
    "debounce_ms": 10_000,
    "fire_on_change": True,
    "hold_ms": HOLD_MS,
}


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--node", default="COM6")
    ap.add_argument("--steady", type=float, default=90.0)
    ap.add_argument("--warm", action="store_true", help="run the attended warm phase after steady")
    ap.add_argument("--warm-within", type=float, default=90.0)
    args = ap.parse_args()

    node = Node(args.node, dry=False)
    r = node.send("set_limits", {"limits": LIMITS}, rid="lim")
    assert r.get("ok"), r
    r = node.send("set_reflex_rules", {"rules": [RULE]}, rid="rules")
    assert r.get("ok") and r["result"].get("loaded") == 1, r
    t0 = node.send("sensor_read", {"sensor": "esp32", "field": "die_temperature"}, rid="t0")
    print(f"rule loaded on {NODE_ID}; die {t0.get('result')} °C; offset {OFFSET_C} °C over a {TAU_S:.0f} s baseline")

    def watch(seconds: float, label: str):
        """Drain the node's USB for `seconds`; return (die-rising reports, temperature samples)."""
        reports, temps = [], []
        end = time.time() + seconds
        next_read = 0.0

        def take(chunk: str):
            if not chunk.startswith("{"):
                return
            try:
                obj = json.loads(chunk)
            except json.JSONDecodeError:
                return
            if obj.get("type") == "reflex" and obj.get("rule_id") == "die-rising":
                reports.append(obj)
                print(f"  [{label}] die-rising report: applied={obj.get('applied')} error={obj.get('error')}")

        while time.time() < end:
            node.ser.timeout = 0.05
            for _ in range(20):
                take(node.ser.readline().decode(errors="replace").strip())
            node.ser.timeout = 2
            if time.time() >= next_read:
                # A report arriving during the send lands in `node.unsolicited`.
                mark = len(node.unsolicited)
                r = node.send("sensor_read", {"sensor": "esp32", "field": "die_temperature"}, rid="t")
                for line in node.unsolicited[mark:]:
                    take(line)
                if r.get("ok"):
                    temps.append((round(time.time(), 1), float(r["result"])))
                next_read = time.time() + 5.0
            time.sleep(0.2)
        return reports, temps

    print(f"steady: watching {args.steady:.0f}s; the rule must not fire")
    steady_reports, steady_temps = watch(args.steady, "steady")
    tmin = min(t for _, t in steady_temps) if steady_temps else None
    tmax = max(t for _, t in steady_temps) if steady_temps else None
    steady_ok = not steady_reports
    print(f"  die ranged {tmin}–{tmax} °C; {len(steady_reports)} report(s) → {'PASS' if steady_ok else 'MISS'}")

    warm_ok = None
    warm_reports, warm_temps = [], []
    if args.warm:
        input(f"\nwarm: put a thumb on the XIAO's metal can and hold it; press Enter to start watching ({args.warm_within:.0f}s) ")
        warm_reports, warm_temps = watch(args.warm_within, "warm")
        wmax = max(t for _, t in warm_temps) if warm_temps else None
        warm_ok = len(warm_reports) == 1 and warm_reports[0].get("applied") is True
        print(f"  die peaked at {wmax} °C; {len(warm_reports)} report(s) (want exactly one, applied) → {'PASS' if warm_ok else 'MISS'}")
        node.send("gpio_write", {"pin": LED_PIN, "value": 1}, rid="led-off")

    node.send("set_reflex_rules", {"rules": []}, rid="clear")
    passed = steady_ok and (warm_ok is not False)
    print("PASS" if passed else "FAIL")

    stamp = time.strftime("%Y%m%d-%H%M%S")
    out = ROOT / "results" / f"bench_baseline_rule-{stamp}.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps({
        "stamp": stamp, "node": args.node, "rule": RULE, "passed": passed,
        "steady": {"seconds": args.steady, "ok": steady_ok, "reports": steady_reports, "temps": steady_temps},
        "warm": None if not args.warm else {"ok": warm_ok, "reports": warm_reports, "temps": warm_temps},
    }, indent=2))
    print(f"record -> {out}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
