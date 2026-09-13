#!/usr/bin/env python3
"""bench_boot_hydrate.py — a node that announces a boot gets its limits back.

Runs against the LIVE brain (it must be up with [lora_gateway] and
[mesh_supervisor] enabled and hold the base station). Nothing here opens the
base; the node is reset over its USB and the result is read from the brain's
own world memory, read-only.

  1. read the node's current boot_id over USB (`capabilities`, quiet open);
  2. reset the node: the ESP32-S3's native USB-Serial-JTAG resets when RTS is
     asserted while DTR is not — the same gesture `bench_run.Node` avoids;
  3. confirm a new boot_id over USB, then close USB so the node runs alone;
  4. poll world.db for `mesh.<node>.limits_pushed` carrying the new boot_id, and
     for the node's next beacon for that boot to have dropped `policy:
     "deny-all"` — the node's own word that limits landed. The reply to
     `lim<boot_id hex>` is reported but not required: the mesh loses about a
     frame in three, and the beacon says the same thing 30 s later regardless.

    python scripts/bench_boot_hydrate.py --node COM6 [--within 120]

PASS = the push lands and the node's beacon stops saying deny-all, for the boot
the reset caused.
Records to results/bench_boot_hydrate-<stamp>.json.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import sqlite3
import sys
import time

import serial

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from bench_run import Node  # noqa: E402
from bench_die_rule import NODE_ID  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent
WORLD_DB = pathlib.Path(os.environ["APPDATA"]) / "thewriterben" / "oh-ben-claw" / "data" / "world.db"


def current(entity: str):
    con = sqlite3.connect(f"file:{WORLD_DB}?mode=ro", uri=True)
    try:
        row = con.execute(
            "select value_json, valid_from from world_facts where entity=? and valid_to is null "
            "order by id desc limit 1",
            (entity,),
        ).fetchone()
    finally:
        con.close()
    return (json.loads(row[0]), row[1]) if row else (None, None)


def boot_id_over_usb(port: str) -> int:
    node = Node(port, dry=False)
    r = node.send("capabilities", {}, rid="cap")
    node.ser.close()
    assert r.get("ok"), r
    return int(r["result"]["boot_id"])


def reset_node(port: str):
    """RTS asserted, DTR not: the S3's auto-reset circuit in silicon."""
    s = serial.Serial()
    s.port, s.baudrate, s.timeout = port, 115200, 0.05
    s.dtr = False
    s.rts = True
    s.open()
    s.dtr = False
    s.rts = True
    time.sleep(0.2)
    s.rts = False
    time.sleep(0.1)
    s.close()


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--node", default="COM6")
    ap.add_argument("--within", type=float, default=90.0)
    args = ap.parse_args()

    before = boot_id_over_usb(args.node)
    print(f"{NODE_ID} boot_id before: {before:#010x}")
    reset_node(args.node)
    print("reset pulsed; waiting for the node to re-enumerate ...")
    after = None
    for _ in range(30):
        time.sleep(1.0)
        try:
            after = boot_id_over_usb(args.node)
            break
        except Exception:  # noqa: BLE001 — port not back yet
            continue
    if after is None or after == before:
        print(f"FAIL: node did not reset (after={after})")
        return 1
    print(f"boot_id after: {after:#010x} — USB closed, node runs alone")

    want_id = f"lim{after:08x}"
    t0 = time.time()
    pushed = reply = told_beacon = None
    while time.time() - t0 < args.within:
        p, _ = current(f"mesh.{NODE_ID}.limits_pushed")
        if p and p.get("boot_id") == after:
            pushed = p
        r, _ = current(f"mesh.{NODE_ID}.cmd_result")
        # Retries carry `r{n}` (the mesh loses about a frame in three).
        if r and str(r.get("id", "")).startswith(want_id):
            reply = r
        # The node's own word: a beacon for this boot with no `policy` field
        # means limits landed, whether or not the reply survived the air.
        b, _ = current(f"mesh.{NODE_ID}.beacon")
        if b and b.get("boot_id") == after and "policy" not in b:
            told_beacon = b
        if pushed and told_beacon:
            break
        time.sleep(2.0)
    elapsed = round(time.time() - t0, 1)
    applied = None
    if reply:
        try:
            applied = json.loads(reply["result"]).get("applied") if isinstance(reply.get("result"), str) else reply.get("result", {}).get("applied")
        except (ValueError, AttributeError):
            applied = None
    print(f"limits_pushed for {after:#010x}: {'yes' if pushed else 'NO'} {json.dumps(pushed)[:160] if pushed else ''}")
    print(f"node reply to {want_id}*: {'yes' if reply else 'no (lost on air, or not yet)'} applied={applied}")
    print(f"node beacon for this boot without policy=deny-all: {'yes' if told_beacon else 'NO'}")
    passed = bool(pushed) and "error" not in (pushed or {}) and told_beacon is not None
    print(("PASS" if passed else "FAIL") + f" ({elapsed}s)")

    stamp = time.strftime("%Y%m%d-%H%M%S")
    out = ROOT / "results" / f"bench_boot_hydrate-{stamp}.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps({
        "stamp": stamp, "node": args.node, "boot_id_before": before, "boot_id_after": after,
        "pushed": pushed, "reply": reply, "applied": applied, "told_beacon": told_beacon,
        "seconds": elapsed, "passed": passed,
    }, indent=2))
    print(f"record -> {out}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
