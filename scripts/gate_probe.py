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


def probe(node, label, requires_refusal, expect_reason, **args):
    """One rule, checked by the reason it fired -- not merely that it fired.

    The first version of this script asked only `ok is False`. After the node
    rebooted mid-run and reverted to deny-all, every probe was refused with
    `pin 3 not in allow-list` and the script reported that the value-range and
    rate-limit rules had both fired correctly. They had not run at all. A gate
    that refuses everything passes a test that only counts refusals, which makes
    such a test worthless exactly when the node is at its most broken.
    """
    rid = f"gate-{label}"
    r = node.send("gpio_write", args, rid=rid)
    ok = r.get("ok") is True
    refused = r.get("refused") is True
    reason = r.get("error") or ""
    print(f"  {label:<28} {args}")
    print(f"      ok={ok} refused={refused} error={reason!r}")

    if not requires_refusal:
        verdict = "allowed, as the policy requires" if ok else "!! refused an ALLOWED write"
        print(f"      {verdict}")
        return 0 if ok else 1

    if ok:
        print(f"      {FAIL}")
        return 1
    if expect_reason not in reason:
        print(f"      !! refused, but for the WRONG REASON -- expected "
              f"{expect_reason!r}. This rule was never exercised; something")
        print("         else refused first, most likely a reboot back to deny-all.")
        return 1
    print(f"      {PASS}")
    return 0


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
    failures += probe(node, "pin-8-not-in-list", True, "not in allow-list", pin=8, value=1)
    time.sleep(0.8)
    failures += probe(node, "pin-99-in-no-list", True, "not in allow-list", pin=99, value=1)
    time.sleep(0.8)
    failures += probe(node, "value-5-out-of-range", True, "out of range", pin=3, value=5)
    time.sleep(0.8)

    allowed = probe(node, "pin-3-allowed", False, "", pin=3, value=1) == 0
    failures += 0 if allowed else 1
    r2 = node.send("gpio_write", {"pin": 3, "value": 0}, rid="gate-rate-2")
    print(f"  {'rate-limit-immediate':<28} {{'pin': 3, 'value': 0}}")
    reason2 = r2.get("error") or ""
    print(f"      ok={r2.get('ok')} refused={r2.get('refused')} error={reason2!r}")
    if r2.get("ok") is True:
        print(f"      {FAIL}")
        failures += 1
    elif "rate limit" not in reason2:
        print("      !! refused, but not by the rate limit. This rule was never")
        print("         exercised.")
        failures += 1
    else:
        print(f"      {PASS}")

    node.send("gpio_write", {"pin": 3, "value": 0}, rid="gate-cleanup")

    print()
    if failures:
        print(f"  {failures} of the gate's rules did not do what the policy says.")
        print("  If several were refused for the wrong reason, suspect a reboot")
        print("  back to deny-all rather than a broken rule -- scripts/")
        print("  reboot_amnesia.py distinguishes those two.")
        return 1
    print("  Every rule fired, and each for its own reason.")
    return 0 if allowed else 1


if __name__ == "__main__":
    raise SystemExit(main())
