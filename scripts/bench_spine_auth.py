#!/usr/bin/env python3
"""bench_spine_auth.py — SPINE-AUTH step 4 on the bench: authenticated frames between two Heltecs.

Both stations on PC USB, opened with DTR/RTS low so opening a port does not
reset the board (a reset is a test of its own, below). Three checks, each a
number rather than a feeling:

  observe      Listen on both consoles for --seconds. Every `SPINE ◄` line
               must carry `ctr=`, counters per source must be strictly
               increasing, and there must be no `REJECTED` line. Keepalives
               run every 5 s in each direction, so 60 s is ~12 frames each way.

  reboot-gap   DTR-reset the bridge (gw-40, COM5) and measure the bounded
               gap SPINE-REPLAY.md §3 promises: after a restart the receiver
               resumes at its persisted ceiling (h + M, M = 8) and refuses
               up to M legitimate frames while the sender catches up. Counts
               the base keepalives the bridge logs as received before and
               after, and reports the gap in counters. Also confirms the
               base keeps accepting the rebooted bridge (its counter resumed
               above anything the base had seen).

  wrong-root   Run after flashing gw-40 with a *different* OBC_SPINE_ROOT:
               every frame each side hears from the other must be REJECTED
               as a bad tag, and nothing may be accepted. Reflash with the
               right root afterwards and run `observe` again.

    python scripts/bench_spine_auth.py observe --seconds 60
    python scripts/bench_spine_auth.py reboot-gap
    python scripts/bench_spine_auth.py wrong-root --seconds 40

Records every line both ports printed to results/bench_spine_auth-<mode>-<stamp>.json.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
import time

import serial

ROOT = pathlib.Path(__file__).resolve().parent.parent

RX = re.compile(r"SPINE ◄ src=([0-9A-F]{2}) seq=(\d+) ctr=(\d+) rssi=(-?\d+) dBm")
REJECTED = re.compile(r"SPINE ◄ REJECTED src=([0-9A-F]{2}) ctr=(\d+) rssi=(-?\d+) dBm \((\d+) B\): (.*)")
TX = re.compile(r"SPINE ► \((\w+)\) seq=(\d+)")
RESUMED = re.compile(r"frame counter resumed at (\d+)")
FINGERPRINT = re.compile(r"root fingerprint ([0-9a-f]{4})")


class Station:
    """One Heltec console, opened without touching DTR/RTS."""

    def __init__(self, name: str, port: str):
        self.name, self.port = name, port
        s = serial.Serial()
        s.port, s.baudrate, s.timeout = port, 115200, 0.05
        s.dtr = False
        s.rts = False
        s.open()
        s.dtr = False
        s.rts = False
        self.ser = s
        self.buf = b""
        self.lines: list[str] = []

    def read(self) -> list[str]:
        chunk = self.ser.read(4096)
        if not chunk:
            return []
        self.buf += chunk
        *whole, self.buf = self.buf.split(b"\n")
        out = []
        for w in whole:
            line = w.decode(errors="replace").rstrip()
            # strip ANSI colour from EspLogger
            line = re.sub(r"\x1b\[[0-9;]*m", "", line)
            if line:
                out.append(line)
                self.lines.append(f"{time.time():.2f} {self.name} {line[:260]}")
        return out

    def reset(self):
        """Reboot the board through the CP2102 auto-reset circuit: EN is pulled
        low while RTS is asserted and DTR is not (the esptool sequence). A DTR
        pulse alone does nothing — the first run of this measured a 'gap' that
        was an ordinary collision on a board that had never restarted."""
        self.ser.dtr = False
        self.ser.rts = True
        time.sleep(0.2)
        self.ser.rts = False
        self.buf = b""


def listen(stations: list[Station], seconds: float, stop=None) -> dict:
    """Collect and classify lines from every station for `seconds` (or until
    `stop(summary)` says so). Returns per-station counts and per-source
    counter sequences."""
    summary = {s.name: {"rx": [], "rejected": [], "tx": 0, "resumed": None, "fingerprint": None}
               for s in stations}
    t0 = time.time()
    while time.time() - t0 < seconds:
        for s in stations:
            for line in s.read():
                st = summary[s.name]
                if m := RX.search(line):
                    st["rx"].append({"src": m.group(1), "seq": int(m.group(2)), "ctr": int(m.group(3)),
                                     "rssi": int(m.group(4)), "t": round(time.time() - t0, 2)})
                elif m := REJECTED.search(line):
                    st["rejected"].append({"src": m.group(1), "ctr": int(m.group(2)), "rssi": int(m.group(3)),
                                           "bytes": int(m.group(4)), "why": m.group(5).strip(),
                                           "t": round(time.time() - t0, 2)})
                elif TX.search(line):
                    st["tx"] += 1
                elif m := RESUMED.search(line):
                    st["resumed"] = int(m.group(1))
                elif m := FINGERPRINT.search(line):
                    st["fingerprint"] = m.group(1)
        if stop is not None and stop(summary):
            break
        time.sleep(0.01)
    summary["_elapsed"] = round(time.time() - t0, 2)
    return summary


def counters_increase(rx: list[dict]) -> tuple[bool, dict]:
    """Per source, counters strictly increasing, and seq == ctr & 0xff."""
    by_src: dict[str, list[int]] = {}
    ok = True
    for r in rx:
        by_src.setdefault(r["src"], []).append(r["ctr"])
        if r["seq"] != r["ctr"] & 0xFF:
            ok = False
    for src, ctrs in by_src.items():
        if any(b <= a for a, b in zip(ctrs, ctrs[1:])):
            ok = False
    return ok, {k: (v[0], v[-1], len(v)) for k, v in by_src.items()}


def report(checks: list[tuple[str, bool, str]]) -> int:
    misses = 0
    for name, ok, detail in checks:
        print(f"[{'PASS' if ok else 'MISS'}] {name}: {detail}")
        misses += 0 if ok else 1
    return misses


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("mode", choices=["observe", "reboot-gap", "wrong-root"])
    ap.add_argument("--base", default="COM3")
    ap.add_argument("--bridge", default="COM5")
    ap.add_argument("--seconds", type=float, default=60.0)
    args = ap.parse_args()

    base = Station("base", args.base)
    bridge = Station("bridge", args.bridge)
    time.sleep(0.3)
    for s in (base, bridge):
        s.ser.reset_input_buffer()

    checks: list[tuple[str, bool, str]] = []
    record: dict = {"mode": args.mode, "base": args.base, "bridge": args.bridge}

    if args.mode == "observe":
        print(f"listening {args.seconds:.0f}s on both consoles ...")
        s = listen([base, bridge], args.seconds)
        record["summary"] = s
        for name in ("base", "bridge"):
            rx, rej = s[name]["rx"], s[name]["rejected"]
            inc, spans = counters_increase(rx)
            checks.append((f"{name} received authenticated frames", len(rx) >= 3,
                           f"{len(rx)} SPINE ◄ lines with ctr= in {s['_elapsed']}s, spans {spans}"))
            checks.append((f"{name} counters strictly increasing per source, seq = ctr & 0xff", inc, f"{spans}"))
            checks.append((f"{name} rejected nothing", not rej, f"{len(rej)} REJECTED lines" +
                           (": " + "; ".join(r["why"] for r in rej[:3]) if rej else "")))

    elif args.mode == "reboot-gap":
        print("phase 1: 20 s of normal traffic ...")
        before = listen([base, bridge], 20.0)
        last_seen_by_bridge = max((r["ctr"] for r in before["bridge"]["rx"] if r["src"] == "D8"), default=None)
        last_bridge_ctr_at_base = max((r["ctr"] for r in before["base"]["rx"] if r["src"] == "40"), default=None)
        checks.append(("bridge heard the base before the reset", last_seen_by_bridge is not None,
                       f"last base ctr accepted by bridge: {last_seen_by_bridge}"))
        print("phase 2: DTR-reset the bridge, then listen until it accepts the base again (≤ 120 s) ...")
        bridge.reset()
        t_reset = time.time()
        after = listen([base, bridge], 120.0,
                       stop=lambda s: any(r["src"] == "D8" for r in s["bridge"]["rx"]))
        record["before"], record["after"] = before, after
        first_after = next((r for r in after["bridge"]["rx"] if r["src"] == "D8"), None)
        base_sent_meanwhile = [r["ctr"] for r in after["base"]["rx"]]  # what base *received*; sends aren't ctr-logged
        if first_after is not None and last_seen_by_bridge is not None:
            gap = first_after["ctr"] - last_seen_by_bridge - 1
            checks.append(("bridge resumed its receive window and re-accepted the base",
                           True, f"first base ctr accepted after reset: {first_after['ctr']}, "
                                 f"{first_after['t']}s after reset; {gap} base frame(s) skipped in between"))
            checks.append(("the post-restart gap is bounded by M = 8", 0 <= gap <= 8, f"gap {gap}"))
        else:
            checks.append(("bridge resumed its receive window and re-accepted the base", False,
                           "no base frame accepted by the bridge within 120 s of the reset"))
        checks.append(("bridge counter resumed above its last (never reissued)",
                       after["bridge"]["resumed"] is not None and last_bridge_ctr_at_base is not None
                       and after["bridge"]["resumed"] >= last_bridge_ctr_at_base,
                       f"resumed at {after['bridge']['resumed']}, base had last accepted bridge ctr {last_bridge_ctr_at_base}"))
        base_rx_bridge_after = [r["ctr"] for r in after["base"]["rx"] if r["src"] == "40"]
        checks.append(("base accepted the rebooted bridge's first frames", len(base_rx_bridge_after) >= 1,
                       f"bridge ctrs accepted by base after reset: {base_rx_bridge_after[:6]}"))
        rej = after["base"]["rejected"] + after["bridge"]["rejected"]
        rej_loud = [r for r in rej]
        checks.append(("no loud rejections across the reset (window gap is silent by design)", not rej_loud,
                       f"{len(rej_loud)} REJECTED lines" + (": " + rej_loud[0]["why"] if rej_loud else "")))
        checks.append(("bridge boot log names the root fingerprint", after["bridge"]["fingerprint"] is not None,
                       f"fingerprint {after['bridge']['fingerprint']}"))
        record["summary"] = {"gap_frames": (first_after["ctr"] - last_seen_by_bridge - 1)
                             if first_after and last_seen_by_bridge is not None else None,
                             "seconds_to_reaccept": first_after["t"] if first_after else None}

    elif args.mode == "wrong-root":
        print(f"listening {args.seconds:.0f}s: the bridge should be running a different root ...")
        s = listen([base, bridge], args.seconds)
        record["summary"] = s
        for name, other in (("base", "40"), ("bridge", "D8")):
            rx = [r for r in s[name]["rx"] if r["src"] == other]
            rej = [r for r in s[name]["rejected"] if r["src"] == other]
            checks.append((f"{name} accepted nothing from the other station", not rx,
                           f"{len(rx)} accepted"))
            checks.append((f"{name} rejected the other station's frames as bad tags", len(rej) >= 3
                           and all("bad tag" in r["why"] for r in rej),
                           f"{len(rej)} REJECTED, reasons: {sorted(set(r['why'] for r in rej))}"))

    misses = report(checks)
    stamp = time.strftime("%Y%m%d-%H%M%S")
    out = ROOT / "results" / f"bench_spine_auth-{args.mode}-{stamp}.json"
    out.parent.mkdir(exist_ok=True)
    record["checks"] = [{"name": n, "ok": ok, "detail": d} for n, ok, d in checks]
    record["lines"] = base.lines + bridge.lines
    out.write_text(json.dumps(record, indent=2))
    print(f"\n{len(checks) - misses}/{len(checks)} checks as stated; record -> {out}")
    return 1 if misses else 0


if __name__ == "__main__":
    sys.exit(main())
