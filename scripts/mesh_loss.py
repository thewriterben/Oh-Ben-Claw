#!/usr/bin/env python3
"""Where do gw-D8's frames go? Every frame the sender logs, followed to its fate.

    python scripts/mesh_loss.py --tx COM3 --rx COM4 --minutes 45
    python scripts/mesh_loss.py --replay results/mesh_loss-<stamp>.log
    python scripts/mesh_loss.py --selftest

The question (BENCH-PINOUT-CARDS.md, 2026-09-17): with relaying off, gw-40 still
missed 4 of 35 and then 6 of 29 of gw-D8's frames. That was read as "a baseline
~10-20% loss source". Those two samples cannot support a number. The 95%
interval on 6/29 alone runs from about 10% to 38%. And `relay_loss.py`, which
produced them, watched only the receiver, so it could not tell a frame that was
never sent from one that was lost on the air. It also counted the one-byte `seq`,
which wraps after 255 frames.

This script watches BOTH consoles at once, stamps every line with host time, and
writes the raw capture to results/ so any analysis can be re-run with --replay.
Each frame gw-D8 logs as transmitted (`SPINE ► (kind) seq=N`) is then given
exactly one fate, in this order:

  received    gw-40 logged `SPINE ◄ src=D8 seq=N` within the match window
  rejected    gw-40 logged `SPINE ◄ REJECTED` while it was on the air
              (with the CRC IRQ masked, a CRC-failed frame surfaced this way)
  crc         gw-40 logged `SX1262 RX: CRC error` while it was on the air
  header      gw-40 logged `SX1262 RX: header error` while it was on the air
  deaf        gw-40 was itself transmitting while it was on the air (half duplex)
  silent      none of the above: nothing on the receiver at all

`crc` and `header` can only appear once gw-40 runs firmware that unmasks those
IRQs (sx1262.rs, 2026-09-25). Before that, CRC failures land in `rejected` and
header failures land in `silent`. Running once on each firmware is the
experiment.

On-air intervals are computed, not assumed. The TX log line is printed after
TxDone, so a frame occupied [t - airtime, t] in host time, with airtime from
the SX126x formula at the firmware's settings (SF7, BW125, CR4/5, 8-symbol
preamble, explicit header, CRC on). Serial latency adds tens of ms of jitter,
so windows get a margin; see ATTRIBUTION_MARGIN_S.

Frames gw-D8 burned a counter on but never sent (`SPINE TX error`, or a gap in
its own seq stream) are reported separately and are NOT counted as air loss.

The brain normally holds gw-40's port, so stop it for the run. Ports are opened
with DTR/RTS low so neither board resets.
"""

from __future__ import annotations

import argparse
import datetime
import json
import math
import os
import re
import statistics
import sys
import threading
import time
from collections import Counter, defaultdict
from dataclasses import dataclass, field

ANSI = re.compile(r"\x1b\[[0-9;]*m")

# ── Radio settings, as programmed in firmware/heltec-lora-linktest/src/sx1262.rs ──
SF = 7
BW_HZ = 125_000
CR = 1  # 4/5
PREAMBLE_SYMBOLS = 8
EXPLICIT_HEADER = True
CRC_ON = True
LOW_DATA_RATE_OPT = False

# ── Frame layout, as encoded in spine.rs: src, seq, ttl, ctr(4), payload, mac(8) ──
FRAME_OVERHEAD = 3 + 4 + 8

# A receiver event is attributed to a transmission if it lands inside the frame's
# on-air interval widened by this much each side. Host timestamps carry serial and
# scheduling jitter of tens of milliseconds; 250 ms is wide enough to absorb it
# and still narrow next to the ~5 s between frames.
ATTRIBUTION_MARGIN_S = 0.25
# gw-40 logs a received frame a few ms after TxDone on gw-D8; allow generous slack
# for USB buffering on either side.
MATCH_WINDOW_S = 2.0


def airtime_s(frame_bytes: int) -> float:
    """SX126x LoRa time on air (datasheet §6.1.4), in seconds."""
    t_sym = (2**SF) / BW_HZ
    de = 1 if LOW_DATA_RATE_OPT else 0
    h = 0 if EXPLICIT_HEADER else 1
    crc = 1 if CRC_ON else 0
    num = 8 * frame_bytes - 4 * SF + 28 + 16 * crc - 20 * h
    n_payload = 8 + max(math.ceil(num / (4 * (SF - 2 * de))) * (CR + 4), 0)
    return (PREAMBLE_SYMBOLS + 4.25 + n_payload) * t_sym


def keepalive_frame_bytes(station: str, body_seq: int) -> int:
    """The keepalive TX line carries no size; rebuild it from main.rs's format."""
    body = f'{{"node_id":"gw-{station}","type":"gw_keepalive","seq":{body_seq}}}'
    return len(body.encode()) + FRAME_OVERHEAD


def wilson(k: int, n: int, z: float = 1.96) -> tuple[float, float]:
    """95% Wilson score interval for k successes in n trials."""
    if n == 0:
        return (0.0, 1.0)
    p = k / n
    denom = 1 + z * z / n
    centre = (p + z * z / (2 * n)) / denom
    half = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / denom
    return (max(0.0, centre - half), min(1.0, centre + half))


# ── Parsing ───────────────────────────────────────────────────────────────────

TX_OK = re.compile(r"SPINE ► \((uart|console|keepalive)\) seq=(\d+)(?: \((\d+) B\))?")
TX_RELAY = re.compile(r"SPINE ⇒ relay src=([0-9A-F]{2}) seq=(\d+)")
TX_ERR = re.compile(r"SPINE TX error: (.*)")
RX_OK = re.compile(
    r"SPINE ◄ src=([0-9A-F]{2}) seq=(\d+) ctr=(\d+) mac=\S+ rssi=(-?\d+) dBm snr=(-?\d+) dB"
)
RX_REJECTED = re.compile(
    r"SPINE ◄ REJECTED src=([0-9A-F]{2}) ctr=(\d+) rssi=(-?\d+) dBm \((\d+) B\): (.*)"
)
RX_CRC = re.compile(r"SX1262 RX: CRC error(?:.*rssi=(-?\d+) dBm)?")
RX_HEADER = re.compile(r"SX1262 RX: header error")
RX_ERR = re.compile(r"SPINE RX error: (.*)")
# Listen-before-talk (heltec main.rs, 2026-09-25): a deferred keepalive, and one
# sent anyway after the deferral cap.
LBT_DEFER = re.compile(r"keepalive deferred: channel busy")
LBT_FORCED = re.compile(r"keepalive sent on a busy channel")


@dataclass
class Tx:
    t: float
    kind: str
    seq: int
    frame_bytes: int
    useq: int = -1  # seq unwrapped across 255 -> 0

    @property
    def air(self) -> tuple[float, float]:
        return (self.t - airtime_s(self.frame_bytes), self.t)


@dataclass
class Station:
    name: str  # "D8", "40"
    tx: list[Tx] = field(default_factory=list)
    relays: list[Tx] = field(default_factory=list)
    tx_errors: list[tuple[float, str]] = field(default_factory=list)
    rx: list[dict] = field(default_factory=list)  # {"t","src","seq","ctr","rssi","snr"}
    rejected: list[dict] = field(default_factory=list)
    crc: list[dict] = field(default_factory=list)
    header: list[float] = field(default_factory=list)
    rx_errors: list[tuple[float, str]] = field(default_factory=list)
    lbt_deferrals: int = 0
    lbt_forced: int = 0


def parse(lines: list[tuple[float, str]], name: str) -> Station:
    """Lines are (host_time, text) from one console, in arrival order."""
    st = Station(name)
    for t, raw in lines:
        line = ANSI.sub("", raw).rstrip()
        if m := TX_OK.search(line):
            kind, seq = m.group(1), int(m.group(2))
            size = (
                int(m.group(3))
                if m.group(3)
                # The keepalive body carries seq+1 of the PREVIOUS frame, which is
                # this frame's seq (main.rs builds it before send_spine! assigns).
                else keepalive_frame_bytes(name, seq)
            )
            st.tx.append(Tx(t, kind, seq, size))
        elif m := TX_RELAY.search(line):
            # Size is the relayed frame's, unknown here; a keepalive is the common case.
            st.relays.append(Tx(t, "relay", int(m.group(2)), keepalive_frame_bytes(m.group(1), 0)))
        elif m := TX_ERR.search(line):
            st.tx_errors.append((t, m.group(1)))
        elif m := RX_REJECTED.search(line):
            st.rejected.append(
                {"t": t, "src": m.group(1), "ctr": int(m.group(2)), "rssi": int(m.group(3)),
                 "bytes": int(m.group(4)), "why": m.group(5).strip()}
            )
        elif m := RX_OK.search(line):
            st.rx.append(
                {"t": t, "src": m.group(1), "seq": int(m.group(2)), "ctr": int(m.group(3)),
                 "rssi": int(m.group(4)), "snr": int(m.group(5))}
            )
        elif m := RX_CRC.search(line):
            st.crc.append({"t": t, "rssi": int(m.group(1)) if m.group(1) else None})
        elif RX_HEADER.search(line):
            st.header.append(t)
        elif m := RX_ERR.search(line):
            st.rx_errors.append((t, m.group(1)))
        elif LBT_DEFER.search(line):
            st.lbt_deferrals += 1
        elif LBT_FORCED.search(line):
            st.lbt_forced += 1
    unwrap(st.tx)
    return st


def unwrap(txs: list[Tx]) -> None:
    """Give each TX a monotonic sequence number across the 255 -> 0 wrap."""
    base, prev = 0, None
    for x in txs:
        if prev is not None and x.seq < prev and prev - x.seq > 128:
            base += 256
        x.useq = base + x.seq
        prev = x.seq


# ── Analysis ──────────────────────────────────────────────────────────────────

FATES = ["received", "rejected", "crc", "header", "deaf", "silent"]


def overlaps(a: tuple[float, float], b: tuple[float, float]) -> bool:
    return a[0] < b[1] and b[0] < a[1]


def fate_of(x: Tx, src: str, rx: Station, rx_busy: list[tuple[float, float]]) -> str:
    for r in rx.rx:
        if r["src"] == src and r["seq"] == x.seq and abs(r["t"] - x.t) <= MATCH_WINDOW_S:
            return "received"
    lo, hi = x.air[0] - ATTRIBUTION_MARGIN_S, x.air[1] + ATTRIBUTION_MARGIN_S
    if any(lo <= e["t"] <= hi for e in rx.rejected):
        return "rejected"
    if any(lo <= e["t"] <= hi for e in rx.crc):
        return "crc"
    if any(lo <= t <= hi for t in rx.header):
        return "header"
    if any(overlaps(x.air, b) for b in rx_busy):
        return "deaf"
    return "silent"


def size_bucket(n: int) -> str:
    return "<=80 B" if n <= 80 else "81-160 B" if n <= 160 else ">160 B"


def analyse(tx: Station, rx: Station) -> dict:
    src = tx.name
    # gw-40's own transmissions make it deaf: its keepalives, console frames and relays.
    rx_busy = [t.air for t in rx.tx] + [t.air for t in rx.relays]
    fates = [(x, fate_of(x, src, rx, rx_busy)) for x in tx.tx]

    def tally(rows):
        c = Counter(f for _, f in rows)
        n = len(rows)
        lost = n - c["received"]
        lo, hi = wilson(lost, n)
        return {
            "sent": n,
            "lost": lost,
            "loss_pct": round(100 * lost / n, 1) if n else None,
            "loss_ci95_pct": [round(100 * lo, 1), round(100 * hi, 1)],
            "fates": {f: c[f] for f in FATES},
        }

    by_kind = defaultdict(list)
    by_size = defaultdict(list)
    for x, f in fates:
        by_kind[x.kind].append((x, f))
        by_size[size_bucket(x.frame_bytes)].append((x, f))

    # Frames D8 burned a counter on but did not log as sent: gaps in its own stream.
    useqs = [x.useq for x in tx.tx]
    never_sent = sum(b - a - 1 for a, b in zip(useqs, useqs[1:]) if b - a > 1)

    # Loss over time, in 5-minute bins, to separate a steady rate from bursts.
    timeline = []
    if fates:
        t0 = fates[0][0].t
        bins = defaultdict(list)
        for x, f in fates:
            bins[int((x.t - t0) // 300)].append((x, f))
        for b in sorted(bins):
            rows = bins[b]
            lost = sum(1 for _, f in rows if f != "received")
            timeline.append({"minute": b * 5, "sent": len(rows), "lost": lost})

    got = [r for r in rx.rx if r["src"] == src]
    link = {}
    if got:
        link = {
            "rssi_dbm": {"min": min(r["rssi"] for r in got),
                         "median": statistics.median(r["rssi"] for r in got),
                         "max": max(r["rssi"] for r in got)},
            "snr_db": {"min": min(r["snr"] for r in got),
                       "median": statistics.median(r["snr"] for r in got),
                       "max": max(r["snr"] for r in got)},
        }

    # Receiver trouble that no D8 frame accounts for: other transmitters, or noise.
    attributed = set()
    for x, _ in fates:
        lo, hi = x.air[0] - ATTRIBUTION_MARGIN_S, x.air[1] + ATTRIBUTION_MARGIN_S
        for i, e in enumerate(rx.rejected):
            if lo <= e["t"] <= hi:
                attributed.add(("rej", i))
        for i, e in enumerate(rx.crc):
            if lo <= e["t"] <= hi:
                attributed.add(("crc", i))
        for i, t in enumerate(rx.header):
            if lo <= t <= hi:
                attributed.add(("hdr", i))
    unattributed = {
        "rejected": len(rx.rejected) - sum(1 for k, _ in attributed if k == "rej"),
        "crc": len(rx.crc) - sum(1 for k, _ in attributed if k == "crc"),
        "header": len(rx.header) - sum(1 for k, _ in attributed if k == "hdr"),
    }

    return {
        "src": src,
        "overall": tally(fates),
        "by_kind": {k: tally(v) for k, v in sorted(by_kind.items())},
        "by_size": {k: tally(v) for k, v in sorted(by_size.items())},
        "timeline_5min": timeline,
        "sender_side": {"tx_errors": len(tx.tx_errors), "seq_gaps_never_sent": never_sent,
                        "lbt_deferrals": tx.lbt_deferrals, "lbt_forced": tx.lbt_forced},
        "receiver_side": {"rx_errors": len(rx.rx_errors),
                          "lbt_deferrals": rx.lbt_deferrals, "lbt_forced": rx.lbt_forced,
                          "rejected_reasons": dict(Counter(e["why"] for e in rx.rejected)),
                          "unattributed": unattributed},
        "link_when_received": link,
        "lost_frames": [
            {"t": round(x.t, 2), "kind": x.kind, "seq": x.seq, "bytes": x.frame_bytes, "fate": f}
            for x, f in fates if f != "received"
        ],
    }


def report(a: dict) -> str:
    o = a["overall"]
    out = [f"=== gw-{a['src']} -> receiver: {o['sent']} frames sent ===",
           f"  loss {o['lost']}/{o['sent']} = {o['loss_pct']}%   "
           f"(95% CI {o['loss_ci95_pct'][0]}-{o['loss_ci95_pct'][1]}%)",
           "  fates: " + ", ".join(f"{k} {v}" for k, v in o["fates"].items())]
    for title, group in (("by kind", a["by_kind"]), ("by size", a["by_size"])):
        out.append(f"  {title}:")
        for k, v in group.items():
            out.append(f"    {k:10s} {v['lost']:3d}/{v['sent']:<4d} {v['loss_pct']}%  "
                       f"CI {v['loss_ci95_pct'][0]}-{v['loss_ci95_pct'][1]}%  "
                       + " ".join(f"{f}={n}" for f, n in v["fates"].items() if n and f != "received"))
    s, r = a["sender_side"], a["receiver_side"]
    out.append(f"  sender: {s['tx_errors']} TX errors, {s['seq_gaps_never_sent']} seq never logged as sent, "
               f"LBT deferred {s['lbt_deferrals']} / forced {s['lbt_forced']}")
    out.append(f"  receiver: LBT deferred {r['lbt_deferrals']} / forced {r['lbt_forced']}")
    out.append(f"  receiver: {r['rx_errors']} RX errors; rejected reasons {r['rejected_reasons'] or '{}'}; "
               f"unattributed {r['unattributed']}")
    if a["link_when_received"]:
        l = a["link_when_received"]
        out.append(f"  link when received: rssi {l['rssi_dbm']}, snr {l['snr_db']}")
    out.append("  5-min bins: " + "  ".join(f"{b['minute']}m {b['lost']}/{b['sent']}"
                                             for b in a["timeline_5min"]))
    return "\n".join(out)


# ── Capture ───────────────────────────────────────────────────────────────────

def quiet_open(port: str):
    import serial  # only needed for a live run

    s = serial.Serial()
    s.port, s.baudrate, s.timeout = port, 115200, 0.05
    s.dtr = s.rts = False
    s.open()
    s.dtr = s.rts = False
    return s


def capture(ports: dict[str, str], seconds: float, log_path: str) -> dict[str, list]:
    lines: dict[str, list] = {role: [] for role in ports}
    lock = threading.Lock()
    stop = time.time() + seconds

    def reader(role: str, port: str, log):
        s = quiet_open(port)
        s.reset_input_buffer()
        buf = b""
        while time.time() < stop:
            chunk = s.read(4096)
            if not chunk:
                continue
            t = time.time()
            buf += chunk
            *whole, buf = buf.split(b"\n")
            for w in whole:
                text = w.decode("utf-8", errors="replace").rstrip("\r")
                if not text.strip():
                    continue
                with lock:
                    lines[role].append((t, text))
                    log.write(f"{t:.3f}\t{role}\t{text}\n")
        s.close()

    with open(log_path, "w", encoding="utf-8") as log:
        threads = [threading.Thread(target=reader, args=(r, p, log), daemon=True)
                   for r, p in ports.items()]
        for th in threads:
            th.start()
        last = time.time()
        while any(th.is_alive() for th in threads):
            time.sleep(1)
            if time.time() - last >= 60:
                last = time.time()
                with lock:
                    sent = sum(1 for _, l in lines["tx"] if TX_OK.search(ANSI.sub("", l)))
                left = max(0, stop - time.time()) / 60
                print(f"  ... {sent} frames logged by the sender, {left:.0f} min left", flush=True)
    return lines


def load(log_path: str) -> dict[str, list]:
    lines: dict[str, list] = {"tx": [], "rx": []}
    with open(log_path, encoding="utf-8") as f:
        for row in f:
            t, role, text = row.rstrip("\n").split("\t", 2)
            lines[role].append((float(t), text))
    return lines


# ── Self-test ─────────────────────────────────────────────────────────────────

def selftest() -> int:
    failures = []

    def check(name, got, want):
        if got != want:
            failures.append(f"{name}: got {got!r}, want {want!r}")

    # Airtime: 10 B at SF7/BW125/CR4-5, 8-symbol preamble, explicit header, CRC on
    # is 41.2 ms by Semtech's own calculator.
    check("airtime 10 B", round(airtime_s(10) * 1000, 1), 41.2)
    check("wilson 0/0", wilson(0, 0), (0.0, 1.0))
    lo, hi = wilson(6, 29)
    check("wilson 6/29 brackets 20.7%", lo < 6 / 29 < hi, True)
    check("keepalive size", keepalive_frame_bytes("D8", 7), len('{"node_id":"gw-D8","type":"gw_keepalive","seq":7}') + 15)

    # A synthetic run. D8 sends seq 253..255, 0..5 (wrap), plus a TX error that burns
    # a counter. Each lost frame is set up to have exactly one fate.
    E = "\x1b[0;32mI (100) heltec: "
    R = "\x1b[0m"
    tx_lines, rx_lines = [], []
    t = 1000.0

    def send(seq, kind="keepalive", size=None):
        nonlocal t
        t += 5.0
        body = f" ({size} B) {{...}}" if size else ""
        tx_lines.append((t, f"{E}SPINE ► ({kind}) seq={seq}{body}{R}"))
        return t

    def heard(when, seq, ctr):
        rx_lines.append((when + 0.02, f"{E}SPINE ◄ src=D8 seq={seq} ctr={ctr} mac=00 rssi=-52 dBm snr=11 dB : {{}}{R}"))

    heard(send(253), 253, 253)
    heard(send(254), 254, 254)
    t_silent = send(255)                                   # silent
    heard(send(0, "uart", 206), 0, 256)                    # wrap; received
    t_rej = send(1, "uart", 206)                           # rejected
    rx_lines.append((t_rej - 0.1, f"W (1) heltec: SPINE ◄ REJECTED src=3F ctr=99 rssi=-50 dBm (206 B): bad tag"))
    t_crc = send(2)                                        # crc
    rx_lines.append((t_crc, "W (1) heltec: SX1262 RX: CRC error — frame arrived corrupt and was dropped (rssi=-51 dBm)"))
    t_hdr = send(3)                                        # header
    rx_lines.append((t_hdr - 0.05, "W (1) heltec: SX1262 RX: header error — frame arrived with a corrupt header and was dropped"))
    t_deaf = send(4)                                       # gw-40 transmitting over it
    rx_lines.append((t_deaf - 0.02, f"{E}SPINE ► (keepalive) seq=17{R}"))
    t += 5.0
    tx_lines.append((t, "I (1) heltec: SPINE TX error: SX1262 TxDone timeout"))  # burns seq 5
    heard(send(6), 6, 262)
    rx_lines.append((5000.0, "W (1) heltec: SX1262 RX: CRC error — frame arrived corrupt and was dropped (rssi=-90 dBm)"))

    d8, gw40 = parse(tx_lines, "D8"), parse(sorted(rx_lines), "40")
    check("unwrap", [x.useq for x in d8.tx], [253, 254, 255, 256, 257, 258, 259, 260, 262])
    a = analyse(d8, gw40)
    check("fates", a["overall"]["fates"],
          {"received": 4, "rejected": 1, "crc": 1, "header": 1, "deaf": 1, "silent": 1})
    check("sent", a["overall"]["sent"], 9)
    check("never sent", {k: a["sender_side"][k] for k in ("tx_errors", "seq_gaps_never_sent")},
          {"tx_errors": 1, "seq_gaps_never_sent": 1})
    lbt = parse([(1.0, "I (1) heltec: keepalive deferred: channel busy (listen-before-talk 1/5), retry in 412 ms"),
                 (2.0, "I (2) heltec: keepalive deferred: channel busy (listen-before-talk 2/5), retry in 333 ms"),
                 (3.0, "I (3) heltec: keepalive sent on a busy channel after 5 deferrals")], "40")
    check("lbt counts", (lbt.lbt_deferrals, lbt.lbt_forced), (2, 1))
    check("unattributed crc", a["receiver_side"]["unattributed"], {"rejected": 0, "crc": 1, "header": 0})
    check("uart by size", a["by_size"][">160 B"]["fates"]["rejected"], 1)
    check("silent one is 255", [f["seq"] for f in a["lost_frames"] if f["fate"] == "silent"], [255])
    check("lost frame times", round(a["lost_frames"][0]["t"], 1), round(t_silent, 1))

    # The replay path reads what capture writes.
    import tempfile

    with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False, encoding="utf-8") as f:
        for role, rows in (("tx", tx_lines), ("rx", sorted(rx_lines))):
            for tt, text in rows:
                f.write(f"{tt:.3f}\t{role}\t{text}\n")
        path = f.name
    back = load(path)
    os.unlink(path)
    rows_by_role = lambda d: {k: sorted(v) for k, v in d.items()}
    check("replay fates", analyse(parse(rows_by_role(back)["tx"], "D8"),
                                  parse(rows_by_role(back)["rx"], "40"))["overall"]["fates"],
          a["overall"]["fates"])

    if failures:
        print("SELFTEST FAILED")
        for f in failures:
            print("  " + f)
        return 1
    print("selftest ok: airtime, Wilson interval, seq unwrap, all six fates, "
          "sender-side gaps, unattributed errors, LBT counts, replay round-trip")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--tx", default="COM3", help="sender's console (gw-D8)")
    ap.add_argument("--rx", default="COM4", help="receiver's console (gw-40)")
    ap.add_argument("--src", default="D8", help="sender station id as it appears in src=")
    ap.add_argument("--rx-name", default="40", help="receiver station id")
    ap.add_argument("--minutes", type=float, default=45.0)
    ap.add_argument("--label", default="", help="free text stored with the run, e.g. the firmware")
    ap.add_argument("--replay", help="analyse a capture log instead of reading ports")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return selftest()

    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "results")
    if args.replay:
        lines, log_path = load(args.replay), args.replay
    else:
        os.makedirs(root, exist_ok=True)
        stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
        log_path = os.path.join(root, f"mesh_loss-{stamp}.log")
        print(f"capturing {args.minutes:.0f} min: sender {args.tx}, receiver {args.rx} -> {log_path}")
        lines = capture({"tx": args.tx, "rx": args.rx}, args.minutes * 60, log_path)

    a = analyse(parse(lines["tx"], args.src), parse(lines["rx"], args.rx_name))
    a["label"] = args.label
    a["capture"] = os.path.basename(log_path)
    print(report(a))
    out = os.path.splitext(log_path)[0] + ".json"
    with open(out, "w", encoding="utf-8") as f:
        json.dump(a, f, indent=2)
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
