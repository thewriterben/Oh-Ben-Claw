#!/usr/bin/env python3
"""bench_descend_lora.py — walkthrough Part B for the spinal tier: `descend` over the air.

Two ports. The node's USB (COM6, VID 0x303a) is used only to push the slot
rule (a rule set never fits a LoRa frame) and to *verify* with reflex_tick
that a threshold moved. The base station's console (COM3, CP210x) is where
the descend lines go: the base frames each onto LoRa (`SPINE ► (console)`),
the bridge gw-40 hands it to the node over UART, and the node's reply comes
back the same way and lands on the base console as `SPINE ◄ src=… : {json}`.

    python scripts/bench_descend_lora.py --node COM6 --base COM3

Records every line both ports printed to results/bench_descend_lora-<stamp>.json.
Nothing is decided here beyond "the reply with this id came back over the air";
a miss is data.
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
from bench_descend import SLOT_RULE, active, fired_ids  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent
NODE_ID = "obc-esp32-s3-001"


class Lines:
    """Whole lines off a port with a short timeout. `readline()` on a port with
    a 50 ms timeout returns a *fragment* when the timeout lands mid-line — and
    the base's `SPINE ◄` line with a 121 B payload is long enough that the
    fragment holding the id never holds the marker. Buffer bytes, split on
    newline, hand back only complete lines."""

    def __init__(self, port: serial.Serial):
        self.port = port
        self.buf = b""

    def read(self) -> list[str]:
        chunk = self.port.read(4096)
        if not chunk:
            return []
        self.buf += chunk
        *whole, self.buf = self.buf.split(b"\n")
        return [w.decode(errors="replace").rstrip() for w in whole]


def drain(port: serial.Serial, seconds: float, want_id: str | None = None,
          also: serial.Serial | None = None, also_lines: list | None = None):
    """Read lines for `seconds`; return (lines, reply-json-with-want_id-or-None).

    `also` is the node's USB port, read alongside. Not decoration: the XIAO's
    native USB-Serial-JTAG blocks on write once the host has the port open and
    stops reading, and the node writes every reply to USB *and* UART1 — so a
    script that holds COM6 open and only listens to the base sees the
    over-the-air reply arrive 8–17 s late, or not within the window at all
    (measured 2026-09-12, two runs). Keep the USB side drained and the reply
    reaches the bridge in about a second.
    """
    lines, reply = [], None
    t0 = time.time()
    main_lines, side_lines = Lines(port), (Lines(also) if also is not None else None)
    pending: list[str] = []
    while time.time() - t0 < seconds:
        if side_lines is not None and also_lines is not None:
            also_lines.extend(l[:200] for l in side_lines.read() if l)
        if not pending:
            pending = [l for l in main_lines.read() if l]
            if not pending:
                continue
        raw = pending.pop(0)
        lines.append(f"{time.time()-t0:5.2f} {raw[:260]}")
        if want_id and "SPINE ◄" in raw and " : " in raw:
            payload = raw.split(" : ", 1)[1].split("\x1b")[0].strip()
            try:
                obj = json.loads(payload)
            except json.JSONDecodeError:
                continue
            if isinstance(obj, dict) and obj.get("id") == want_id:
                reply = obj
                if isinstance(reply.get("result"), str):
                    try:
                        reply["result"] = json.loads(reply["result"])
                    except json.JSONDecodeError:
                        pass
                # keep reading a moment for the rssi line / echoes, then stop
                t0 = time.time() - (seconds - 1.0)
    return lines, reply


def main() -> int:
    # The base console speaks in arrows (SPINE ►/◄); a cp1252 Windows console
    # cannot print them and the first run died on the first one.
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--node", default="COM6")
    ap.add_argument("--base", default="COM3")
    ap.add_argument("--wait", type=float, default=10.0, help="seconds to wait for an over-the-air reply")
    ap.add_argument("--retries", type=int, default=2,
                    help="resend an unanswered descend up to this many times (idempotent; attempts recorded)")
    ap.add_argument("--min-seq", type=int, default=0,
                    help="wait until the base's keepalive seq reaches this before sending "
                         "(clears the bridge's de-dup ring after a base reboot)")
    ap.add_argument("--no-node-usb", action="store_true",
                    help="node is on a power bank, not the PC: skip the USB push/verify steps "
                         "(the rule must already be loaded) and judge only by the over-the-air replies")
    args = ap.parse_args()

    node = None if args.no_node_usb else Node(args.node, dry=False)
    # Open the base with DTR/RTS held low. A default open toggles DTR, the
    # Heltec's auto-reset circuit reboots it, and its 8-bit spine seq restarts
    # at 0 — after which every command it sends carries a (src, seq) the
    # bridge's 32-entry de-dup ring has already seen from the previous boot,
    # and the bridge drops it as a flood duplicate. Found 2026-09-12: four
    # recorded runs of this script "sent" commands that never left the ring.
    base = serial.Serial()
    base.port, base.baudrate, base.timeout = args.base, 115200, 0.05
    base.dtr = False
    base.rts = False
    base.open()
    base.dtr = False
    base.rts = False
    time.sleep(0.3)
    base.reset_input_buffer()
    if args.min_seq:
        # If the base did reboot recently, let its seq climb clear of the ring.
        print(f"waiting for the base's keepalive seq to reach {args.min_seq} ...")
        t0 = time.time()
        while time.time() - t0 < 300:
            raw = base.readline().decode(errors="replace")
            m = re.search(r"SPINE ► \(keepalive\) seq=(\d+)", raw)
            if m:
                print(f"    base seq {m.group(1)}")
                if int(m.group(1)) >= args.min_seq:
                    break

    record: list[dict] = []
    misses: list[str] = []

    def usb(name, cmd, cmd_args, expect, check):
        if node is None:
            print(f"[SKIP] usb  {name}: node not on USB")
            record.append({"via": "usb", "step": name, "skipped": True})
            return {}
        reply = node.send(cmd, cmd_args, rid=name)
        ok = False
        try:
            ok = bool(check(reply))
        except Exception as e:  # noqa: BLE001
            reply = {"_check_error": str(e), **reply}
        print(f"[{'PASS' if ok else 'MISS'}] usb  {name}: {expect}")
        print(f"        got: {json.dumps(reply)[:240]}")
        record.append({"via": "usb", "step": name, "cmd": cmd, "args": cmd_args, "expect": expect, "ok": ok, "reply": reply})
        if not ok:
            misses.append(name)
        return reply

    def air(name, rid, cmd_args, expect, check):
        # Reply-awaited retry. The mesh has no ACK: a command or its reply can
        # be lost to a plain half-duplex collision (measured ~1 in 5 with the
        # RX and keepalive fixes in place, 2026-09-12). `descend` is idempotent
        # — setting a level twice is the same level — so resending is safe.
        # Each attempt carries its own id so a late reply to an earlier try
        # cannot be mistaken for the current one. Attempts are recorded; a
        # pass on the third try is a pass that says "third try".
        lines, reply, node_lines, attempts = [], None, [], 0
        for attempt in range(args.retries + 1):
            attempts = attempt + 1
            rid_try = rid if attempt == 0 else f"{rid}r{attempt}"
            line = json.dumps({"id": rid_try, "to": NODE_ID, "cmd": "descend", "args": cmd_args},
                              separators=(",", ":"))
            assert len(line) <= 240, f"{len(line)} bytes will not fit a frame"
            base.reset_input_buffer()
            if node is not None:
                node.ser.reset_input_buffer()
                node.ser.timeout = 0.05
            base.write((line + "\n").encode())
            got, reply = drain(base, args.wait, want_id=rid_try,
                               also=None if node is None else node.ser, also_lines=node_lines)
            lines.extend(f"[try {attempts}] {l}" for l in got)
            if node is not None:
                node.ser.timeout = 2
            if reply is not None:
                break
            print(f"        no reply to {rid_try} within {args.wait:.0f}s" +
                  (", retrying" if attempt < args.retries else ""))
        ok = False
        if reply is not None:
            try:
                ok = bool(check(reply))
            except Exception as e:  # noqa: BLE001
                reply = {"_check_error": str(e), **reply}
        rssi = [l for l in lines if "SPINE ◄" in l and '"id":"' + rid in l]
        if reply is not None and rssi:
            m = re.search(r"rssi=(-?\d+) dBm", rssi[0])
            if m:
                reply["_rssi_dbm"] = int(m.group(1))
        if reply is not None:
            reply["_attempts"] = attempts
        print(f"[{'PASS' if ok else 'MISS'}] air  {name}: {expect}  ({len(line)} B sent, {attempts} attempt(s))")
        print(f"        reply: {json.dumps(reply)[:240] if reply else 'NONE within %.0fs' % args.wait}")
        for l in lines:
            if "SPINE" in l:
                print(f"        base: {l[:200]}")
        record.append({"via": "lora", "step": name, "sent": line, "expect": expect, "ok": ok,
                       "attempts": attempts, "reply": reply, "base_lines": lines,
                       "node_usb_lines": node_lines, "rssi_lines": rssi})
        if not ok:
            misses.append(name)
        return reply

    tick = lambda t: {"snapshot": {"sensor.temperature": t}}  # noqa: E731

    usb("push-slot-rule", "set_reflex_rules", {"rules": [SLOT_RULE]}, "loaded 1",
        lambda r: r.get("ok") is True and r["result"].get("loaded") == 1)
    # Levels belong to slots, not rules, and survive a rule push — a level left
    # by an earlier run would make the baseline look like a miss.
    usb("clear-leftover-levels", "descend", {"clear": True}, "active []",
        lambda r: r.get("ok") is True and active(r) == [])
    usb("baseline-65-fires", "reflex_tick", tick(65.0), "'overheat' fires at default 60",
        lambda r: "overheat" in fired_ids(r))
    air("descend-1.0-over-air", "b1", {"m": [[3, 1.0]]}, "reply id b1, active [[3,1.0]]",
        lambda r: r.get("ok") is True and active(r) == [[3, 1.0]])
    usb("verify-65-silent", "reflex_tick", tick(65.0), "NO 'overheat' (threshold now 80)",
        lambda r: "overheat" not in fired_ids(r))
    air("descend-0.0-over-air", "b2", {"m": [[3, 0.0]]}, "reply id b2, active [[3,0.0]]",
        lambda r: r.get("ok") is True and active(r) == [[3, 0.0]])
    usb("verify-45-fires", "reflex_tick", tick(45.0), "'overheat' only (threshold now 40)",
        lambda r: fired_ids(r) == ["overheat"])
    air("bad-slot-refused-over-air", "b3", {"m": [[16, 0.5]]}, "reply id b3, ok:false slot 16",
        lambda r: r.get("ok") is False and "slot 16" in str(r.get("error")))
    air("clear-over-air", "b4", {"clear": True}, "reply id b4, active []",
        lambda r: r.get("ok") is True and active(r) == [])
    usb("verify-65-fires-again", "reflex_tick", tick(65.0), "'overheat' fires at default again",
        lambda r: "overheat" in fired_ids(r))

    stamp = time.strftime("%Y%m%d-%H%M%S")
    out = ROOT / "results" / f"bench_descend_lora-{stamp}.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps({"node": args.node, "base": args.base, "stamp": stamp,
                               "misses": misses, "steps": record}, indent=2))
    print(f"\n{len(record) - len(misses)}/{len(record)} steps as stated; record -> {out}")
    if misses:
        print("misses: " + ", ".join(misses))
    return 1 if misses else 0


if __name__ == "__main__":
    sys.exit(main())
