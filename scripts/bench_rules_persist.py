#!/usr/bin/env python3
"""bench_rules_persist.py — a reset no longer costs the node its rules.

  1. push the die-temperature rules over USB; the reply must say
     `persisted: true`;
  2. reset the node (RTS asserted, DTR not) and reopen USB;
  3. `reflex_tick` with a cold snapshot: `die-cool` must be among the fired —
     the rules came back from NVS without any host push;
  4. with --live, keep USB drained and wait for the node to become whole on
     its own: the brain re-pushes the pin-21 limit over the mesh
     (`hydrate_limits`), and the restored `die-cool` rule then fires
     `applied: true` — a reflex report on USB says so.

    python scripts/bench_rules_persist.py --node COM6 [--live] [--within 150]

Step 4 needs the brain up with [lora_gateway] + [mesh_supervisor] and
`[[safety.limits]]` naming pin 21. Records to results/bench_rules_persist-<stamp>.json.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from bench_run import Node  # noqa: E402
from bench_die_rule import RULES, NODE_ID  # noqa: E402
from bench_boot_hydrate import reset_node  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--node", default="COM6")
    ap.add_argument("--live", action="store_true")
    ap.add_argument("--within", type=float, default=150.0)
    args = ap.parse_args()
    record: dict = {"stamp": time.strftime("%Y%m%d-%H%M%S"), "node": args.node}

    node = Node(args.node, dry=False)
    r = node.send("set_reflex_rules", {"rules": RULES}, rid="push")
    record["push"] = r
    persisted = bool(r.get("ok")) and r["result"].get("persisted") is True
    print(f"[{'PASS' if persisted else 'MISS'}] push: {json.dumps(r.get('result'))}")
    before = node.send("capabilities", {}, rid="cap")["result"]["boot_id"]
    node.ser.close()

    reset_node(args.node)
    print("reset pulsed ...")
    node = None
    for _ in range(30):
        time.sleep(1.0)
        try:
            node = Node(args.node, dry=False)
            break
        except Exception:  # noqa: BLE001
            continue
    assert node is not None, "node did not come back on USB"
    after = node.send("capabilities", {}, rid="cap2")["result"]["boot_id"]
    record["boot_id_before"], record["boot_id_after"] = before, after
    print(f"boot_id {before:#010x} -> {after:#010x}")
    reset_ok = after != before

    # A cold reading: 20 °C is below any slot-0 threshold, so die-cool must fire
    # if — and only if — the rule is loaded.
    t = node.send("reflex_tick", {"snapshot": {"sensor.die_temperature": 20.0}}, rid="tick")
    record["tick"] = t
    fired = [f.get("rule_id") for f in t.get("result", {}).get("fired", [])]
    restored = "die-cool" in fired
    print(f"[{'PASS' if restored else 'MISS'}] after reset, reflex_tick fires {fired}")

    whole = None
    if args.live:
        print(f"live: waiting up to {args.within:.0f}s for the brain's limits and a die-cool applied:true ...")
        node.ser.timeout = 0.05
        t0 = time.time()
        seen = []
        while time.time() - t0 < args.within:
            raw = node.ser.readline().decode(errors="replace").strip()
            if raw.startswith("{"):
                try:
                    obj = json.loads(raw)
                except json.JSONDecodeError:
                    continue
                if obj.get("type") == "reflex" and obj.get("rule_id") in ("die-cool", "die-hot"):
                    seen.append(obj)
                    print(f"  {time.time()-t0:5.1f}s reflex {obj.get('rule_id')} applied={obj.get('applied')} error={obj.get('error')}")
                    if obj.get("applied") is True:
                        break
        whole = any(o.get("applied") is True for o in seen)
        record["live_reports"] = seen
        print(f"[{'PASS' if whole else 'MISS'}] node whole again: {'yes' if whole else 'no applied:true within the window'}")
    node.ser.close()

    passed = persisted and reset_ok and restored and (whole is not False)
    record.update({"persisted": persisted, "reset_ok": reset_ok, "restored": restored, "whole": whole, "passed": passed})
    print("PASS" if passed else "FAIL")
    out = ROOT / "results" / f"bench_rules_persist-{record['stamp']}.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps(record, indent=2))
    print(f"record -> {out}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
