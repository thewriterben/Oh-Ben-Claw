#!/usr/bin/env python3
"""bench_run.py — the protocol half of docs/BENCH-RUN-THE-FOUR-OPEN-CLAIMS.md.

The four open claims need a board. Three of them also need a person watching a
wire, and this script is careful about which is which:

* It sends the commands, captures the raw replies, and writes them down.
* It **asks** for anything only eyes can settle, and never infers it. "The reply
  said refused" is not the same claim as "the pin did not move", and the whole
  reason step 1 exists is that a gate refusing in the log while the pin twitches
  is a real failure mode.

It records what happened rather than deciding whether it was good. A step that
fails is data; the run continues.

    python scripts/bench_run.py                 # auto-detect the port
    python scripts/bench_run.py --port COM6
    python scripts/bench_run.py --dry-run       # simulated node, no hardware

`--dry-run` exists because this script was written without a board to test it
on. It replays a canned node so the prompts, the record and the failure paths
can be exercised before anyone relies on it at a bench. It proves the script
works. It proves nothing whatever about the firmware.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
BAUD = 115200
NODE_ID = "obc-esp32-s3-001"

LIMITS = [
    {
        "node_id": NODE_ID,
        "tool": "gpio_write",
        "allowed_pins": [3, 7],
        "value_min": 0,
        "value_max": 1,
        "min_interval_ms": 500,
    }
]


class Node:
    """A serial link to the node, or a simulation of one."""

    def __init__(self, port: str | None, dry: bool):
        self.dry = dry
        self.log: list[tuple[str, str]] = []
        if dry:
            self.port = "(simulated)"
            self._sim = _Sim()
            return
        import serial  # imported here so --dry-run needs no pyserial

        self.port = port or self._detect(serial)
        self.ser = serial.Serial(self.port, BAUD, timeout=2)
        time.sleep(0.3)
        self.ser.reset_input_buffer()

    @staticmethod
    def _detect(serial) -> str:
        from serial.tools import list_ports

        found = list(list_ports.comports())
        if not found:
            sys.exit(
                "no serial ports found. Plug the board in over USB-C (a data\n"
                "cable, not charge-only) and try again, or pass --port."
            )
        if len(found) == 1:
            print(f"using the only port present: {found[0].device} "
                  f"({found[0].description})")
            return found[0].device
        print("more than one serial port is present:")
        for p in found:
            print(f"  {p.device}  {p.description}")
        sys.exit("pass --port to say which one is the node.")

    def send(self, cmd: str, args: dict | None = None, rid: str | None = None) -> dict:
        req = {"id": rid or cmd, "cmd": cmd, "args": args or {}}
        line = json.dumps(req)
        if self.dry:
            raw = self._sim.handle(req)
        else:
            self.ser.write((line + "\n").encode())
            raw = ""
            deadline = time.time() + 3
            while time.time() < deadline:
                chunk = self.ser.readline().decode(errors="replace").strip()
                if not chunk:
                    continue
                # The node also logs; a reply is the line that parses as JSON.
                if chunk.startswith("{"):
                    raw = chunk
                    break
        self.log.append((line, raw))
        try:
            return json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            return {"_unparsed": raw}


class _Sim:
    """A node that behaves the way the firmware says it does.

    Deliberately not a mock that agrees with everything: it enforces the pushed
    policy, so the refusal steps in a --dry-run go down the refusal path and the
    record shows what a real refusal would look like.
    """

    def __init__(self) -> None:
        self.pins = [21, 3, 7, 8]
        self.vmin, self.vmax = 0, 1
        self.interval = None
        self.last: dict[int, float] = {}

    def handle(self, req: dict) -> str:
        cmd, args = req["cmd"], req.get("args", {})
        rid = req.get("id")
        if cmd in ("capabilities", "announce"):
            return json.dumps({
                "id": rid, "ok": True, "result": {
                    "node_id": NODE_ID, "board": "seeed-xiao-esp32-s3",
                    "firmware_version": "0.4.2", "gpio": self.pins,
                    "i2c_bus": [5, 6], "camera": False, "microphone": True,
                }})
        if cmd == "set_limits":
            for lim in args.get("limits", []):
                if lim.get("tool") == "gpio_write" and lim.get("node_id") in ("", NODE_ID):
                    self.pins = lim.get("allowed_pins") or []
                    self.vmin, self.vmax = lim.get("value_min"), lim.get("value_max")
                    self.interval = lim.get("min_interval_ms")
                    self.last.clear()
                    return json.dumps({"id": rid, "ok": True, "result": {
                        "applied": True, "allowed_pins": self.pins,
                        "value_min": self.vmin, "value_max": self.vmax,
                        "min_interval_ms": self.interval}})
            return json.dumps({"id": rid, "ok": True, "result": {"applied": False}})
        if cmd == "gpio_write":
            pin, value, now = args.get("pin"), args.get("value"), time.time() * 1000
            if pin not in self.pins:
                return json.dumps({"id": rid, "ok": False, "error": "pin not in the allow-list"})
            if self.vmin is not None and not (self.vmin <= value <= self.vmax):
                return json.dumps({"id": rid, "ok": False, "error": "value out of range"})
            if self.interval and pin in self.last and now - self.last[pin] < self.interval:
                return json.dumps({"id": rid, "ok": False, "error": "faster than min_interval_ms"})
            self.last[pin] = now
            return json.dumps({"id": rid, "ok": True, "result": None})
        if cmd == "gpio_read":
            return json.dumps({"id": rid, "ok": True, "result": 0})
        if cmd == "sensor_read":
            if args.get("sensor") == "bme280":
                return json.dumps({"id": rid, "ok": True, "result": 41.7})
            if args.get("field") == "battery_soc":
                return json.dumps({"id": rid, "ok": True, "result": 87.5})
            return json.dumps({"id": rid, "ok": True, "result": 9.79})
        return json.dumps({"id": rid, "ok": False, "error": f"unknown cmd {cmd}"})


def ask(question: str, options: str = "y/n") -> str:
    """Anything only a person can settle. Never inferred from a reply."""
    while True:
        got = input(f"    ?? {question} [{options}] ").strip().lower()
        if got:
            return got


def show(label: str, reply: dict) -> None:
    print(f"    -> {label}: {json.dumps(reply)[:150]}")


def firmware_commit() -> str:
    try:
        out = subprocess.run(
            ["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, timeout=10)
        return out.stdout.strip() or "unknown"
    except Exception:
        return "unknown"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--port")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--out", default="bench-run-record.md")
    a = ap.parse_args()

    if a.dry_run:
        print("\n*** --dry-run: talking to a simulated node. This exercises the")
        print("*** script. It says nothing about the firmware or the hardware.\n")

    node = Node(a.port, a.dry_run)
    rec: dict[str, object] = {
        "date": time.strftime("%Y-%m-%d %H:%M"),
        "firmware commit": firmware_commit(),
        "port": node.port,
        "simulated": a.dry_run,
    }

    print("=" * 68)
    print("  0. What is this node?")
    print("=" * 68)
    caps = node.send("capabilities").get("result", {})
    show("capabilities", caps)
    rec["board reported"] = caps.get("board", "(none)")
    rec["node_id reported"] = caps.get("node_id", "(none)")
    rec["gpio reported"] = caps.get("gpio", "(none)")
    rec["i2c reported"] = caps.get("i2c_bus", "(none)")
    if caps.get("node_id") != NODE_ID:
        print(f"    !! node id is {caps.get('node_id')!r}, not {NODE_ID!r}.")
        print("       The pushed limit will not match and every step below would")
        print("       be testing the boot policy instead. Fix this first.")
    print(f"    The node says it is a {rec['board reported']}, safe pins "
          f"{rec['gpio reported']}, I2C {rec['i2c reported']}.")
    print("    Those came from the node, not from this script. If they disagree")
    print("    with the board in front of you, that disagreement is the finding.")

    print()
    print("=" * 68)
    print("  1. Does a refusal stop the wire moving?")
    print("=" * 68)
    print("  Wire an LED + 330R from GPIO 3 to ground, and a second on GPIO 7.")
    input("  Press Enter when the LEDs are wired. ")

    applied = node.send("set_limits", {"limits": LIMITS}).get("result", {})
    show("set_limits", applied)
    rec["set_limits applied"] = applied.get("applied")
    if not applied.get("applied"):
        print("    !! applied is not true: no limit matched this node. Stop here.")

    print("\n  1a. The control -- this must pass or nothing below means anything.")
    r = node.send("gpio_write", {"pin": 3, "value": 1})
    show("gpio_write pin 3 value 1", r)
    rec["control reply ok"] = r.get("ok")
    rec["control LED lit"] = ask("Did the LED on GPIO 3 light?")
    if rec["control LED lit"].startswith("n"):
        print("    !! With a dark control LED, a dark LED later proves nothing --")
        print("       a refusal and a disconnected wire look identical. Fix the")
        print("       wiring before trusting steps 1b-1d.")

    print("\n  1b. The refusal. Watch the PIN, not the reply.")
    r = node.send("gpio_write", {"pin": 8, "value": 1})
    show("gpio_write pin 8 value 1", r)
    rec["refusal reply refused"] = (r.get("ok") is False)
    rd = node.send("gpio_read", {"pin": 8})
    show("gpio_read pin 8", rd)
    rec["refusal gpio_read"] = rd.get("result")
    rec["refusal pin stayed dark"] = ask("Did GPIO 8 stay dark / not move?")
    rec["refusal measured with"] = ask("Measured with?", "eye/meter/scope")

    print("\n  1c. Without a host. This script talks straight down the serial")
    print("      line with no agent mediating, so this step is already the")
    print("      host-absent case -- provided no agent is running.")
    rec["agent running"] = ask("Is the OBC agent running against this node?", "y/n")
    r = node.send("gpio_write", {"pin": 8, "value": 1}, rid="host-absent")
    show("gpio_write pin 8 (no host)", r)
    rec["refuses without host"] = (r.get("ok") is False)

    print("\n  1d. The rate limit: two writes to pin 3 inside 500 ms.")
    # Wait out the interval first. Step 1a already wrote to pin 3, so without
    # this the *first* write of the pair gets refused as too-fast and the test
    # is vacuous -- both refused proves nothing about rate limiting. The dry run
    # caught exactly that, because piped answers arrive instantly where a person
    # would have taken several seconds to reply.
    interval_ms = applied.get("min_interval_ms") or 500
    print(f"      (waiting {interval_ms} ms so the pair starts clean)")
    time.sleep(interval_ms / 1000 + 0.2)
    r1 = node.send("gpio_write", {"pin": 3, "value": 0}, rid="rate-1")
    r2 = node.send("gpio_write", {"pin": 3, "value": 1}, rid="rate-2")
    show("first (after the interval)", r1)
    show("second (immediate)", r2)
    rec["rate limit first allowed"] = (r1.get("ok") is True)
    rec["rate limit refused the second"] = (r2.get("ok") is False)
    if r1.get("ok") is not True:
        print("    !! the first write was refused too, so this step tested")
        print("       nothing. Note it rather than reading the second refusal")
        print("       as evidence.")
    rec["rate limit pin held"] = ask("Did the LED hold steady rather than flicker?")

    print()
    print("=" * 68)
    print("  2. Does the BME280 work on the corrected 5/6 bus?")
    print("=" * 68)
    if ask("Is a BME280 wired to SDA=GPIO5, SCL=GPIO6?") .startswith("n"):
        rec["bme280"] = "not wired -- not tested"
        print("    skipped.")
    else:
        r = node.send("sensor_read", {"sensor": "bme280", "field": "humidity"})
        show("humidity", r)
        rec["humidity at rest"] = r.get("result")
        print("    Breathe on the sensor, then press Enter.")
        input("    ")
        r2 = node.send("sensor_read", {"sensor": "bme280", "field": "humidity"})
        show("humidity after breath", r2)
        rec["humidity after breath"] = r2.get("result")
        rec["humidity responded"] = ask("Did it climb and fall back?")
        print("    A plausible constant number is what a stub read looks like.")

    print()
    print("=" * 68)
    print("  3. Addresses and decode")
    print("=" * 68)
    r = node.send("sensor_read", {"sensor": "max17048", "field": "battery_soc"})
    show("battery_soc", r)
    rec["battery_soc"] = r.get("result")
    rec["battery_soc plausible"] = ask("Is that a plausible state of charge?")
    r = node.send("sensor_read", {"sensor": "mpu6050", "field": "accel_z"})
    show("accel_z (flat)", r)
    rec["accel_z flat"] = r.get("result")
    print("    Turn the board over, then press Enter.")
    input("    ")
    r = node.send("sensor_read", {"sensor": "mpu6050", "field": "accel_z"})
    show("accel_z (inverted)", r)
    rec["accel_z inverted"] = r.get("result")
    rec["accel sign flipped"] = ask("Did the sign flip?")

    print()
    print("=" * 68)
    print("  4. Waveshare camera connector -- SETTLED 2026-08-21, not asked")
    print("=" * 68)
    print("  It has none; its only FPC connector is the screen's. Observed")
    print("  directly, no power needed. `camera.rs` was the sole source saying")
    print("  otherwise and has been corrected.")
    rec["waveshare FPC connector"] = "none (settled 2026-08-21 by observation)"

    print()
    print("  Anything that failed *plausibly* -- looked fine, wasn't?")
    rec["plausible failures"] = input("    ").strip() or "(none noted)"

    out = ROOT / a.out
    lines = ["# Bench run record", ""]
    if a.dry_run:
        lines += ["> **SIMULATED RUN.** No hardware was involved. This record",
                  "> proves the script runs; it is not evidence about anything.", ""]
    for k, v in rec.items():
        lines.append(f"- **{k}**: {v}")
    lines += ["", "## Raw exchange", "", "```"]
    for sent, got in node.log:
        lines += [f"> {sent}", f"< {got}"]
    lines += ["```", ""]
    out.write_text("\n".join(lines), encoding="utf-8")
    print(f"\n  Record written to {out}")
    print("  The raw exchange is in it. Paste the record into the PR that")
    print("  updates the 'not verified' rows -- with the commit, not the word")
    print("  'worked'.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
