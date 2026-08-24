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


def _decode_result(raw):
    """The firmware's `result` is always a String. Give back what it holds.

    `Response` in `firmware/obc-esp32-s3/src/main.rs` declares `result: String`,
    so `capabilities` arrives as a JSON *document inside a JSON string* and a
    sensor reading arrives as `"41.7"`, not `41.7`. This script read `result`
    as though it were already an object and crashed on the first real node it
    ever met -- while `--dry-run` passed, because the simulator had been written
    from the documentation instead of from the wire format. A simulator that
    disagrees with the firmware validates the script against a fiction.
    """
    if not isinstance(raw, str):
        return raw
    if raw == "":
        return None
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return raw


class Node:
    """A serial link to the node, or a simulation of one."""

    def __init__(self, port: str | None, dry: bool):
        self.dry = dry
        self.log: list[tuple[str, str]] = []
        # Lines the node said that were not answers to anything we asked:
        # beacons, link_state, reflex ticks. Kept rather than dropped -- they
        # are the node's own account of what it was doing during the run.
        self.unsolicited: list[str] = []
        self.seq = 0
        if dry:
            self.port = "(simulated)"
            self._sim = _Sim()
            return
        try:
            import serial  # imported here so --dry-run needs no pyserial
        except ModuleNotFoundError:
            sys.exit(
                "pyserial is not importable from this interpreter.\n"
                f"  interpreter: {sys.executable}\n"
                "  install it into THIS one, not whichever python is on PATH\n"
                "  in another window:\n\n"
                f'    "{sys.executable}" -m pip install pyserial\n\n'
                "  It can be installed and still not import: a --user install\n"
                "  lands in %APPDATA%\\Python\\PythonXY\\site-packages, which is\n"
                "  skipped inside a virtualenv or when PYTHONNOUSERSITE is set.\n"
                "  The line above sidesteps both by naming the interpreter."
            )

        self.port = port or self._detect(serial)
        self.ser = serial.Serial(self.port, BAUD, timeout=2)
        time.sleep(0.3)
        self.ser.reset_input_buffer()

    @staticmethod
    def _kind(p) -> str:
        """What the USB descriptor says this port physically is.

        The node is an ESP32-S3 talking over its *native* USB-Serial-JTAG, so it
        enumerates under Espressif's own VID. A USB-UART bridge chip cannot be
        the node: the XIAO has no bridge fitted. On this bench the bridges are
        the Heltecs (`BENCH-PINOUT-CARDS.md` Card 0), and mistaking one for the
        node is not hypothetical -- on 2026-08-22 a build was flashed to the
        LoRa base station because it was the only port present.
        """
        vid = getattr(p, "vid", None)
        if vid == 0x303A:
            return "native"          # Espressif USB-Serial-JTAG -- could be the node
        if vid in (0x10C4, 0x1A86, 0x0403, 0x067B):
            return "bridge"          # CP210x / CH34x / FTDI / Prolific
        return "unknown"

    @classmethod
    def _detect(cls, serial) -> str:
        from serial.tools import list_ports

        found = list(list_ports.comports())
        if not found:
            sys.exit(
                "no serial ports found. Plug the board in over USB-C (a data\n"
                "cable, not charge-only) and try again, or pass --port."
            )
        for p in found:
            print(f"  {p.device}  {p.description}  [{cls._kind(p)}]")

        native = [p for p in found if cls._kind(p) == "native"]
        if len(native) == 1:
            print(f"using {native[0].device}: the only native USB-Serial-JTAG port.")
            return native[0].device
        if len(native) > 1:
            sys.exit("more than one native USB port present. Pass --port.")

        # Nothing that could be the node. Say what these ports are instead of
        # picking one because it is the only one.
        sys.exit(
            "none of these ports is an ESP32-S3 native USB port.\n"
            "Every port above is a USB-UART bridge chip (CP210x/CH34x/FTDI) or\n"
            "unrecognised. The node is a XIAO ESP32-S3, which has no bridge\n"
            "fitted -- it enumerates under Espressif's VID 0x303A. A CP210x on\n"
            "this bench is one of the Heltecs; `BENCH-PINOUT-CARDS.md` Card 0\n"
            "records the base station `gw-D8` on COM3.\n\n"
            "Plug the XIAO in (data cable, not charge-only). If it still does\n"
            "not appear, that is the finding -- do not fall back to a bridge\n"
            "port, and do not flash one: it is a different board with a\n"
            "different pin map and its own firmware.\n\n"
            "--port overrides this if you know better."
        )

    def send(self, cmd: str, args: dict | None = None, rid: str | None = None) -> dict:
        # Ids must be unique per request. Defaulting an id to the command name
        # made two consecutive `gpio_write` calls indistinguishable on the wire,
        # so a stale or duplicated reply could be matched to the wrong write --
        # in the one step where "which write did this answer" is the entire
        # question. The counter makes every request its own.
        self.seq += 1
        req = {"id": f"{rid or cmd}#{self.seq}", "cmd": cmd, "args": args or {}}
        line = json.dumps(req)
        if self.dry:
            # The simulator interleaves a telemetry line before its answer, the
            # way the node does, so --dry-run actually exercises the id match
            # instead of agreeing that there is nothing to match against.
            raw = ""
            for chunk in self._sim.handle(req):
                try:
                    obj = json.loads(chunk)
                except json.JSONDecodeError:
                    continue
                if not isinstance(obj, dict) or obj.get("id") != req["id"]:
                    self.unsolicited.append(chunk)
                    continue
                raw = chunk
                break
        else:
            self.ser.write((line + "\n").encode())
            raw = ""
            deadline = time.time() + 3
            while time.time() < deadline:
                chunk = self.ser.readline().decode(errors="replace").strip()
                if not chunk.startswith("{"):
                    continue
                # A reply is the line whose `id` is the one we just sent.
                #
                # Matching "the first line that parses as JSON" was wrong, and
                # wrong in the worst available way. This node talks unprompted:
                # `beacon` every ~30 s, `link_state`, and a `reflex` line every
                # ~10 s while no host is attached. Every one of those is JSON on
                # the same wire. Taking the first one as the answer attributes
                # somebody else's sentence to this command -- which, in a
                # procedure that exists to stop a reply being mistaken for a
                # wire, is the same error one level down. Observed 2026-08-22:
                # gpio_read replies came back as 'done', the *previous*
                # gpio_write's result, with every reply shifted by one.
                try:
                    obj = json.loads(chunk)
                except json.JSONDecodeError:
                    continue
                if not isinstance(obj, dict) or obj.get("id") != req["id"]:
                    self.unsolicited.append(chunk)
                    continue
                raw = chunk
                break
        self.log.append((line, raw))
        try:
            reply = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            return {"_unparsed": raw}
        if isinstance(reply, dict) and "result" in reply:
            reply["result"] = _decode_result(reply["result"])
        return reply


class _Sim:
    """A node that behaves the way the firmware says it does.

    Deliberately not a mock that agrees with everything: it enforces the pushed
    policy, so the refusal steps in a --dry-run go down the refusal path and the
    record shows what a real refusal would look like.

    It must also *speak* the way the firmware speaks. `Response.result` is a
    `String`, so every reply below wraps its payload with `json.dumps` a second
    time and refusals carry `refused: true`. Until 2026-08-22 this class emitted
    bare objects and numbers, so --dry-run exercised a protocol the node does
    not use and the script crashed the first time it met real hardware.
    """

    @staticmethod
    def _ok(rid, payload) -> str:
        """`ok` reply. `payload` goes inside the string, as the firmware does."""
        return json.dumps({"id": rid, "ok": True, "result": json.dumps(payload)})

    @staticmethod
    def _refuse(rid, why: str) -> str:
        """Track 0 refusing. `refused` distinguishes policy from malfunction."""
        return json.dumps({"id": rid, "ok": False, "result": "",
                           "refused": True, "error": why})

    def __init__(self) -> None:
        self.pins = [21, 3, 7, 8]
        self.vmin, self.vmax = 0, 1
        self.interval = None
        self.last: dict[int, float] = {}
        self.tick = 0

    def _chatter(self) -> list[str]:
        """What the node says when nobody asked.

        The real node emits `link_state`, `beacon` and a `reflex` line on its own
        schedule, on the same wire as the replies. A simulator that only ever
        speaks when spoken to lets a request/response bug through, which is
        exactly what happened on 2026-08-22.
        """
        self.tick += 1
        if self.tick % 2:
            return [json.dumps({"node_id": NODE_ID, "ts_ms": self.tick * 1000,
                                "type": "beacon"})]
        return [json.dumps({
            "action": {"reason": "host link lost — entering offline safing",
                       "type": "escalate"},
            "applied": False, "error": None, "node_id": NODE_ID,
            "rule_id": "safe-link-offline", "ts_ms": self.tick * 1000,
            "type": "reflex"})]

    def handle(self, req: dict) -> list[str]:
        return self._chatter() + [self._answer(req)]

    def _answer(self, req: dict) -> str:
        cmd, args = req["cmd"], req.get("args", {})
        rid = req.get("id")
        if cmd in ("capabilities", "announce"):
            return self._ok(rid, {
                "node_id": NODE_ID, "board": "seeed-xiao-esp32-s3",
                "firmware_version": "0.4.2", "gpio": self.pins,
                "i2c_bus": [5, 6], "camera": False, "microphone": True,
                "edge_agent": True, "transport": "usb-serial-jtag", "wifi": True,
                # The real node also carries a `tools` array. Not reproduced
                # here: nothing in this procedure reads it, and inventing a
                # plausible copy is how the shape drifted in the first place.
            })
        if cmd == "set_limits":
            for lim in args.get("limits", []):
                if lim.get("tool") == "gpio_write" and lim.get("node_id") in ("", NODE_ID):
                    self.pins = lim.get("allowed_pins") or []
                    self.vmin, self.vmax = lim.get("value_min"), lim.get("value_max")
                    self.interval = lim.get("min_interval_ms")
                    self.last.clear()
                    return self._ok(rid, {
                        "applied": True, "allowed_pins": self.pins,
                        "value_min": self.vmin, "value_max": self.vmax,
                        "min_interval_ms": self.interval})
            return self._ok(rid, {"applied": False})
        if cmd == "gpio_write":
            pin, value, now = args.get("pin"), args.get("value"), time.time() * 1000
            if pin not in self.pins:
                return self._refuse(rid, "pin not in the allow-list")
            if self.vmin is not None and not (self.vmin <= value <= self.vmax):
                return self._refuse(rid, "value out of range")
            if self.interval and pin in self.last and now - self.last[pin] < self.interval:
                return self._refuse(rid, "faster than min_interval_ms")
            self.last[pin] = now
            return self._ok(rid, None)
        if cmd == "gpio_read":
            return self._ok(rid, 0)
        if cmd == "sensor_read":
            if args.get("sensor") == "bme280":
                return self._ok(rid, 41.7)
            if args.get("field") == "battery_soc":
                return self._ok(rid, 87.5)
            return self._ok(rid, 9.79)
        return json.dumps({"id": rid, "ok": False, "result": "",
                           "error": f"unknown cmd {cmd}"})


def ask(question: str, options: str = "y/n") -> str:
    """Anything only a person can settle. Never inferred from a reply."""
    while True:
        got = input(f"    ?? {question} [{options}] ").strip().lower()
        if got:
            return got


def show(label: str, reply: dict) -> None:
    print(f"    -> {label}: {json.dumps(reply)[:150]}")


# A pin driven high sits at the rail; a pin held low sits at ground. Anything
# in between is a pin that is not being driven at all, and the meter is reading
# the air. That third case is the one worth naming, because on 2026-08-21 a
# jumper on GPIO 43 read HIGH purely because an idle UART TX floats there, and
# it was taken for a pin that had moved.
HIGH_MIN_V = 2.0
LOW_MAX_V = 0.5


def volts(gpio: int) -> float:
    """A number from the meter. Not a verdict."""
    while True:
        raw = input(f"    ?? DC volts on GPIO {gpio} (black lead on GND): ")
        raw = raw.strip().rstrip("vV").strip()
        try:
            return float(raw)
        except ValueError:
            print("       A number, e.g. 3.28 or 0.00. mV: type 0.002, not 2.")


def observe(instrument: str, gpio: int, expect: str) -> tuple[bool, str]:
    """What the pin is doing, and whether that is what was expected.

    `expect` is "high" or "low". With a meter the person supplies a reading and
    this decides what the reading means, which is arithmetic. Asking "did it
    stay dark?" instead asks the person to do the deciding, and what comes back
    is a verdict with no measurement under it -- the record then says `y`,
    which is unfalsifiable a month later. A voltage is not.
    """
    if instrument.startswith("m"):
        v = volts(gpio)
        if v >= HIGH_MIN_V:
            state = "high"
        elif v <= LOW_MAX_V:
            state = "low"
        else:
            state = "neither"
        if state == "neither":
            print(f"    !! {v:.2f} V is neither driven high nor held low. If this")
            print("       pin has no 330R to ground, an undriven pin reads")
            print("       whatever the air gives it, and that is not a")
            print("       measurement of anything. Fit the resistor and repeat.")
        return state == expect, f"{v:.3f} V ({state})"

    if instrument.startswith("s"):
        seen = input(f"    ?? Scope on GPIO {gpio} -- what does the trace do? ").strip()
        agree = ask(f"Is that {expect.upper()} and steady?")
        return agree.startswith("y"), f"scope: {seen or '(nothing written down)'}"

    want = "lit" if expect == "high" else "dark"
    got = ask(f"Is the LED on GPIO {gpio} {want}?")
    ok = got.startswith("y")
    return ok, f"LED {want if ok else 'NOT ' + want} (by eye)"


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
    ap.add_argument(
        "--probe", action="store_true",
        help="ask the node what it is, print the answer, and stop. No wiring, "
             "no prompts, no record. Use it when the boot banner is not "
             "readable -- on a board whose only console is the same "
             "USB-Serial-JTAG the firmware takes over for commands, it is not.")
    ap.add_argument(
        "--sections", default="1,2,3",
        help="which sections to run, comma-separated (default 1,2,3). "
             "Section 1 needs LEDs and resistors on GPIO 3/7/8; sections 2 "
             "and 3 need I2C modules on GPIO 5/6. On a 400-point breadboard "
             "both rigs at once is a crowd, so run them as two passes and "
             "keep both records. Section 0 -- what the node says it is -- "
             "always runs: a record that cannot name the node is not a record.")
    ap.add_argument("--out", default="bench-run-record.md")
    a = ap.parse_args()

    try:
        want = {int(s) for s in a.sections.split(",") if s.strip()}
    except ValueError:
        print(f"--sections {a.sections!r}: expected numbers like 1,2,3")
        return 2
    if not want <= {1, 2, 3}:
        print(f"--sections: no such section {sorted(want - {1, 2, 3})}")
        return 2
    if not want:
        print("--sections selected nothing.")
        return 2

    if a.dry_run:
        print("\n*** --dry-run: talking to a simulated node. This exercises the")
        print("*** script. It says nothing about the firmware or the hardware.\n")

    node = Node(a.port, a.dry_run)
    rec: dict[str, object] = {
        "date": time.strftime("%Y-%m-%d %H:%M"),
        "firmware commit": firmware_commit(),
        "port": node.port,
        "simulated": a.dry_run,
        "sections run": ",".join(str(s) for s in sorted(want)),
    }

    print("=" * 68)
    print("  0. What is this node?")
    print("=" * 68)
    caps = node.send("capabilities").get("result")
    if not isinstance(caps, dict):
        # Never index a reply whose shape you have not checked. The first real
        # node this script met returned a JSON string and it died on .get().
        if caps is not None:
            print(f"    !! `result` is a {type(caps).__name__}, not an object: {caps!r}")
        caps = {}
    show("capabilities", caps)
    rec["board reported"] = caps.get("board", "(none)")
    rec["node_id reported"] = caps.get("node_id", "(none)")
    rec["gpio reported"] = caps.get("gpio", "(none)")
    rec["i2c reported"] = caps.get("i2c_bus", "(none)")
    if caps.get("node_id") != NODE_ID:
        print(f"    !! node id is {caps.get('node_id')!r}, not {NODE_ID!r}.")
        print("       The pushed limit will not match and every step below would")
        print("       be testing the boot policy instead.")
        print()
        print("    STOPPING. This used to be a warning and the run carried on,")
        print("    which on 2026-08-22 produced a full transcript of results")
        print("    whose meaning depended on an identity the node never gave.")
        print("    A run that cannot say which node it talked to is not a")
        print("    weaker run; it is not a run.")
        print()
        print("    If the node answered nothing at all, something else may hold")
        print("    the port (Ctrl+C the espflash monitor), or it may still be")
        print("    booting -- wait a second and try again.")
        return 2
    print(f"    The node says it is a {rec['board reported']}, safe pins "
          f"{rec['gpio reported']}, I2C {rec['i2c reported']}.")
    print("    Those came from the node, not from this script. If they disagree")
    print("    with the board in front of you, that disagreement is the finding.")

    # The node's own answer is the only way to tell which build is on it. The
    # 4/5 bus and GPIO 6 as an output are the pre-2026-08-21 firmware: on that
    # build the sensor pads are not the bus the firmware drives, so every
    # reading is a stub, and GPIO 6 is an output while the corrected build has
    # it free.
    stale = []
    if caps.get("i2c_bus") == [4, 5]:
        stale.append("I2C on 4/5 (corrected build drives 5/6)")
    if isinstance(caps.get("gpio"), list) and 6 in caps["gpio"]:
        stale.append("GPIO 6 still an output")
    if stale:
        print()
        print("    !! THIS IS OLD FIRMWARE: " + "; ".join(stale))
        print(f"       reported version {caps.get('firmware_version')!r}.")
        print("       Reflash before going further. Sensor readings from this")
        print("       build are stubs that look like readings.")

    if a.probe:
        print()
        print("    verbatim exchange:")
        for sent, got in node.log:
            print(f"      >> {sent}")
            print(f"      << {got or '(nothing)'}")
        print()
        if not caps:
            print("    The node did not answer. That is a finding too: either the")
            print("    firmware is not running, something else holds the port")
            print("    (the espflash monitor does -- Ctrl+C it), or the command")
            print("    channel is not up. It is NOT evidence about the pin map.")
            return 1
        ok = caps.get("node_id") == NODE_ID
        print(f"    --probe only. Nothing was wired, nothing was written, no")
        print(f"    record file. node_id {'matches' if ok else 'DOES NOT match'}"
              f" {NODE_ID!r}.")
        if stale:
            print("    Exiting non-zero on the firmware, not the node id.")
        return 0 if (ok and not stale) else 1

    print()
    if 1 not in want:
        print("=" * 68)
        print("  1. Refusals and the wire -- NOT SELECTED")
        print("=" * 68)
        print("  Skipped by --sections. No LEDs or resistors are needed")
        print("  for this run. Nothing below infers anything from that:")
        print("  a section that did not run is not a section that passed.")
        rec["section 1"] = "not run (--sections)"
    else:
        print("=" * 68)
        print("  1. Does a refusal stop the wire moving?")
        print("=" * 68)
        # Do not let an unrecognised answer fall through to the LED path. It
        # would run the whole section asking about lamps that are not on the
        # board, and the record would name an instrument nobody used.
        while True:
            instrument = ask("Watching the pins with?", "meter/led/scope")
            instrument = {"m": "meter", "l": "led", "e": "led",
                          "s": "scope"}.get(instrument[0], "")
            if instrument:
                break
            print("       meter, led or scope.")
        rec["section 1 instrument"] = instrument
        print()
        if instrument.startswith("m"):
            print("  METER. No LEDs. Each pin needs ONE 330R from the pin to the")
            print("  ground rail -- the resistor, not the LED, is the part that")
            print("  matters: it defines the pin when nothing is driving it. A")
            print("  bare pin with a meter on it reads the air, and the air has")
            print("  already been mistaken for a signal on this bench once.")
            print("    GPIO 3  -- the CONTROL. In the pushed table: must read ~3.3 V.")
            print("    GPIO 8  -- the REFUSAL. NOT in the pushed table: must stay ~0 V")
            print("               while step 1b writes to it.")
            print("    GPIO 7  -- optional second allowed pin. Nothing needs it.")
            print("  Black lead on GND and leave it there; move only the red probe.")
            print()
            print("  What a meter cannot see: a pulse shorter than its update rate.")
            print("  If the gate refuses and the pin twitches for a millisecond,")
            print("  a steady 0.00 V is what you will read. If your meter has")
            print("  MIN/MAX or peak-hold, arm it before 1b -- that is the one")
            print("  setting that turns this into a check for a transient.")
            input("  Press Enter when the resistors are in. ")
        else:
            print("  Wire, each through 330R to ground:")
            print("    GPIO 3  -- the CONTROL. In the pushed table, so it must light.")
            print("    GPIO 8  -- the REFUSAL. An output at boot but NOT in the pushed")
            print("               table, so it must stay dark. Step 1b writes to it,")
            print("               and a bare pin cannot tell you it stayed dark.")
            print("    GPIO 7  -- optional second allowed pin.")
            input("  Press Enter when the LEDs are wired. ")

        # Prove BOTH LEDs before the restricted policy is pushed.
        #
        # This used to lean on the boot policy: OUTPUT_PINS was [21, 3, 7, 8] and a
        # fresh node would drive any of them. As of 2026-08-22 it will not -- the
        # node boots DENY-ALL and refuses every pin until a host says otherwise,
        # because the old boot policy was wider than any pushed table and every
        # reset silently restored it. So the wiring proof now pushes its own
        # temporary table that opens 3 and 8, and the real restricted table follows.
        #
        # The point of the step is unchanged and is the reason it exists: step 1b
        # asks you to observe GPIO 8 not moving, and a pin that reads 0 V proves a
        # refusal only if that same pin has been seen to reach 3.3 V.
        print("\n  1-pre. Prove the wiring, with a table that deliberately opens pin 8.")
        warmup = [{
            "node_id": NODE_ID, "tool": "gpio_write",
            "allowed_pins": [3, 8], "value_min": 0, "value_max": 1,
            "min_interval_ms": None,
        }]
        w = node.send("set_limits", {"limits": warmup}, rid="pre-open").get("result")
        show("set_limits (wiring proof: 3 and 8 open)", w)
        if not (isinstance(w, dict) and w.get("applied")):
            print("    !! the warm-up table did not apply, so nothing below is wired")
            print("       proof. Stop and fix that first.")
        for pin, role in ((3, "control"), (8, "refusal")):
            r = node.send("gpio_write", {"pin": pin, "value": 1}, rid=f"pre-{pin}-on")
            show(f"gpio_write pin {pin} value 1", r)
            if r.get("ok") is not True:
                print(f"    !! pin {pin} was refused while the warm-up table opened")
                print("       it. That is not the state this test assumes. Stop.")
            ok, detail = observe(instrument, pin, "high")
            rec[f"pre-check pin {pin} ({role}) driven"] = detail
            node.send("gpio_write", {"pin": pin, "value": 0}, rid=f"pre-{pin}-off")
            if not ok:
                print(f"    !! GPIO {pin} did not go high with the write ALLOWED.")
                print("       Fix the wiring now. Every later observation of this pin")
                print("       would otherwise be unreadable: a refusal and a dead")
                print("       wire look identical.")

        # The pre-check just wrote to pin 3. If the gate carries its last-write
        # timestamps across set_limits, step 1a's write would be refused as
        # too-fast and would read as a failed control. Wait out the interval rather
        # than assume the push clears it -- the simulator clears it, which is
        # exactly the kind of agreement that has already been wrong once today.
        time.sleep(0.8)

        applied = node.send("set_limits", {"limits": LIMITS}).get("result")
        if not isinstance(applied, dict):
            print(f"    !! set_limits `result` is not an object: {applied!r}")
            applied = {}
        show("set_limits", applied)
        rec["set_limits applied"] = applied.get("applied")
        if not applied.get("applied"):
            print("    !! applied is not true: no limit matched this node. Stop here.")

        print("\n  1a. The control -- this must pass or nothing below means anything.")
        r = node.send("gpio_write", {"pin": 3, "value": 1})
        show("gpio_write pin 3 value 1", r)
        rec["control reply ok"] = r.get("ok")
        ctrl_ok, detail = observe(instrument, 3, "high")
        rec["control pin measured"] = detail
        if not ctrl_ok:
            print("    !! The control pin did not move with the write ALLOWED. A")
            print("       pin that reads flat later then proves nothing -- a refusal")
            print("       and a disconnected wire read identically. Fix the wiring")
            print("       before trusting steps 1b-1d.")

        print("\n  1b. The refusal. Watch the PIN, not the reply.")
        r = node.send("gpio_write", {"pin": 8, "value": 1}, rid="refusal")
        show("gpio_write pin 8 value 1", r)
        rec["refusal reply refused"] = (r.get("ok") is False)
        rec["refusal reply refused flag"] = r.get("refused")
        if r.get("ok") is True:
            print("    !! THE GATE ALLOWED IT. pin 8 is not in the pushed")
            print("       allowed_pins, set_limits said applied:true, and the node")
            print("       took the write anyway. This is the property the safety")
            print("       case calls load-bearing. Finish the run -- the remaining")
            print("       steps say how much of the gate is inert -- then stop and")
            print("       treat this as the result.")
        rd = node.send("gpio_read", {"pin": 8})
        show("gpio_read pin 8", rd)
        rec["refusal gpio_read"] = rd.get("result")
        held, detail = observe(instrument, 8, "low")
        rec["refusal pin measured"] = detail
        rec["refusal pin stayed put"] = held
        if instrument.startswith("m"):
            rec["refusal transient checked"] = ask(
                "Was the meter in MIN/MAX or peak-hold for that write?")
            if rec["refusal transient checked"].startswith("n"):
                print("    !! Then this reading rules out a pin that MOVED AND STAYED.")
                print("       It does not rule out a pin that pulsed. Say that in the")
                print("       record rather than letting a steady 0.00 V stand for a")
                print("       claim it cannot make.")
        elif instrument.startswith("l"):
            rec["refusal transient checked"] = "no -- an LED cannot show a short pulse"

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
        # The pair writes 0 and then 1. If the rate limit refused the second,
        # pin 3 is sitting LOW; if it let it through, pin 3 is HIGH. So the pin
        # settles into whichever state the gate actually decided, and reading it
        # is a check on the replies rather than a second opinion about them.
        #
        # Not a check for a glitch: neither an LED nor a meter can see a 500 ms
        # write that was immediately overwritten. This asks the smaller question
        # it can actually answer -- does the wire agree with what the node said?
        expect = "high" if r2.get("ok") is True else "low"
        agrees, detail = observe(instrument, 3, expect)
        rec["rate limit pin settled at"] = detail
        rec["rate limit pin agrees with the replies"] = agrees
        if not agrees:
            print(f"    !! The replies say pin 3 should be {expect.upper()} and it is")
            print("       not. One of the two is wrong, and the wire is the one")
            print("       that cannot lie about itself. This is the finding.")

        if (rec.get("refusal reply refused") is False
                or rec.get("rate limit refused the second") is False):
            print()
            print("  " + "=" * 66)
            print("  The pushed policy is not being enforced.")
            print("  " + "=" * 66)
            print("  set_limits reported the table back and the node then ignored")
            print("  it. Run `python scripts\\gate_probe.py` for which of the gate's")
            print("  three rules -- allow-list, value range, rate limit -- still")
            print("  fire. That is the finding of this run; the sensor sections")
            print("  below are unaffected but secondary.")
            print()

    print()
    print("=" * 68)
    print("  2. Does the BME280 work on the corrected 5/6 bus?")
    print("=" * 68)
    if 2 not in want:
        rec["bme280"] = "not run (--sections)"
        print("    Not selected by --sections. Skipped.")
    elif ask("Is a BME280 wired to SDA=GPIO5, SCL=GPIO6?").startswith("n"):
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
    print("  3. Addresses and decode -- NEEDS TWO MORE MODULES")
    print("=" * 68)
    if 3 not in want:
        rec["addresses and decode"] = "not run (--sections)"
        print("  Not selected by --sections. Skipped.")
        print()
        return _write_record(node, rec, a)
    print("  This section is not about the ESP32. It reads two separate I2C")
    print("  boards that have to be on the bus already:")
    print("    MAX17048 fuel gauge at 0x36 -- and a LiPo on its battery pads,")
    print("      or the state of charge it reports is meaningless.")
    print("    MPU6050 accelerometer at 0x68 -- which you have to be able to")
    print("      pick up and turn over, so mount it where the wires allow that.")
    print("  Both share SDA=GPIO5 / SCL=GPIO6 with the BME280.")
    print()
    print("  Until 2026-08-22 this section simply began, with no warning that")
    print("  it needed hardware nobody had been told to fit.")
    if ask("Are the MAX17048 and MPU6050 both wired?").startswith("n"):
        rec["addresses and decode"] = "not wired -- not tested"
        print("    skipped. Nothing here is inferred from their absence.")
    else:
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

    return _write_record(node, rec, a)


def _write_record(node: "Node", rec: dict, a) -> int:
    """Section 4 and the record file. Reached from every exit that ran at all.

    A run that stops early because a section was not selected still has to
    write what it did observe, and still has to carry section 4's settled
    fact. The alternative -- returning straight out of section 3 -- silently
    drops both, and a missing record reads exactly like a run that was never
    made.
    """
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
