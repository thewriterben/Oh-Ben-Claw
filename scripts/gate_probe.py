#!/usr/bin/env python3
"""Ask the Track 0 gate, directly, which of its rules it is actually enforcing.

Written 2026-08-22, after a bench run in which `set_limits` returned
`applied:true` with `allowed_pins [3,7]` and the node then accepted a write to
pin 8 anyway, twice, plus a rate-limited pair inside the interval. The runner
found it; this script exists to say precisely how much of the gate is inert
rather than leaving that to inference from one transcript.

Every request carries a unique id. `bench_run.py` defaults an id to the command
name, so two consecutive `gpio_write` calls are indistinguishable on the wire --
which is survivable there and not survivable here, because the whole question is
which reply belongs to which write.

Nothing is inferred from an LED. Each probe states what the policy requires and
what the node did.
"""

import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bench_run import Node, LIMITS, NODE_ID  # noqa: E402

PASS = "gate refused, as the policy requires"
FAIL = "!! GATE ALLOWED IT -- the policy is not being enforced"


def probe(node, label, requires_refusal, **args):
    rid = f"gate-{label}"
    r = node.send("gpio_write", args, rid=rid)
    ok = r.get("ok") is True
    refused = r.get("refused") is True
    verdict = (FAIL if ok else PASS) if requires_refusal else (
        "allowed, as the policy requires" if ok else "!! refused an ALLOWED write")
    print(f"  {label:<28} {args}")
    print(f"      ok={ok} refused={refused} error={r.get('error')!r}")
    print(f"      {verdict}")
    return ok


def main() -> int:
    node = Node(None, False)

    caps = node.send("capabilities").get("result")
    print(f"\n  node: {caps.get('node_id') if isinstance(caps, dict) else caps!r}")
    if not isinstance(caps, dict) or caps.get("node_id") != NODE_ID:
        print("  !! the node did not identify itself. Everything below would be")
        print("     testing an unknown policy. Stopping.")
        return 2

    print("\n  Pushing the bench limit table.")
    applied = node.send("set_limits", {"limits": LIMITS}, rid="gate-push").get("result")
    print(f"      {applied}")
    if not (isinstance(applied, dict) and applied.get("applied")):
        print("  !! set_limits did not apply. Stopping.")
        return 2

    print("\n  With allowed_pins [3, 7], range 0..1, min_interval 500 ms:\n")
    time.sleep(0.8)

    failures = 0
    failures += probe(node, "pin-8-not-in-list", True, pin=8, value=1)
    time.sleep(0.8)
    failures += probe(node, "pin-99-in-no-list", True, pin=99, value=1)
    time.sleep(0.8)
    failures += probe(node, "value-5-out-of-range", True, pin=3, value=5)
    time.sleep(0.8)

    allowed = probe(node, "pin-3-allowed", False, pin=3, value=1)
    r2 = node.send("gpio_write", {"pin": 3, "value": 0}, rid="gate-rate-2")
    print(f"  {'rate-limit-immediate':<28} {{'pin': 3, 'value': 0}}")
    print(f"      ok={r2.get('ok')} refused={r2.get('refused')} error={r2.get('error')!r}")
    if r2.get("ok") is True:
        print(f"      {FAIL}")
        failures += 1
    else:
        print(f"      {PASS}")

    node.send("gpio_write", {"pin": 3, "value": 0}, rid="gate-cleanup")

    print()
    if failures:
        print(f"  {failures} of the gate's rules did not fire.")
        print("  The node reported the policy and did not apply it. This is the")
        print("  property docs/SAFETY-CASE.md calls load-bearing.")
        return 1
    print("  Every rule fired. The gate enforces what it reported.")
    return 0 if allowed else 1


if __name__ == "__main__":
    raise SystemExit(main())
