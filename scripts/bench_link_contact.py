#!/usr/bin/env python3
"""bench_link_contact.py — does a mesh command count as host contact?

Before 2026-09-13 the node's link-silence clock was reset only by USB bytes,
so a node with USB closed, commanded every few seconds over LoRa, measured
silence from the moment USB closed and reported `link_state: offline` for
good — `safe-link-offline` fired on the wrong input. This bench measures the
fix:

  1. open the node over USB, confirm it answers, close USB (t = 0);
  2. listen on the base for the `link_state offline` that silence *should*
     produce ~30 s later (DEFAULT_LINK_TIMEOUT_MS) — silence is still silence;
  3. send one `gpio_read` over the mesh and expect `link_state online` within
     --online-within seconds;
  4. keep sending a command every --period seconds for --hold seconds and
     expect no further `offline`.

    python scripts/bench_link_contact.py --node COM6 --base COM3

Pass = offline seen in step 2, online seen in step 3, no offline in step 4.
The pre-fix firmware passes step 2 and fails step 3. Records to
results/bench_link_contact-<stamp>.json.
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
from bench_die_rule import NODE_ID  # noqa: E402
from bench_descend_lora import drain  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent
# The node's `json!` sorts keys, so `"state"` precedes `"type"` on the wire;
# match the field, not an assumed order. `silence_ms` is unique to link_state.
STATE_RE = re.compile(r'"state":"(offline|online)"')


def link_states(lines: list[str]) -> list[tuple[float, str]]:
    out = []
    for l in lines:
        if "SPINE ◄" not in l or '"silence_ms"' not in l:
            continue
        m = STATE_RE.search(l)
        if m:
            out.append((float(l.split()[0]), m.group(1)))
    return out


def probe(base: serial.Serial, i: int, wait: float, also=None, also_lines=None):
    rid = f"lc{int(time.time()) % 100000}{i}"
    line = json.dumps({"id": rid, "to": NODE_ID, "cmd": "gpio_read", "args": {"pin": 21}},
                      separators=(",", ":"))
    base.write((line + "\n").encode())
    return drain(base, wait, want_id=rid, also=also, also_lines=also_lines)


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--node", default="COM6")
    ap.add_argument("--base", default="COM3")
    ap.add_argument("--silence", type=float, default=45.0, help="s to wait for the offline edge (timeout is 30 s)")
    ap.add_argument("--online-within", type=float, default=8.0)
    ap.add_argument("--period", type=float, default=10.0)
    ap.add_argument("--hold", type=float, default=60.0)
    ap.add_argument("--watch-usb", action="store_true",
                    help="keep the node's USB open and drained (no bytes sent, so silence still "
                         "accrues) and record link_state from that side too — the node's own "
                         "account, to tell 'never went offline' from 'the mesh lost the report'")
    ap.add_argument("--label", default="")
    args = ap.parse_args()

    node = Node(args.node, dry=False)
    fw = node.send("capabilities", {}, rid="cap").get("result", {})
    version = fw.get("firmware_version") if isinstance(fw, dict) else None
    if args.watch_usb:
        node.ser.timeout = 0.05
        usb, usb_lines = node.ser, []
        print(f"{NODE_ID} answers over USB (firmware {version}); USB stays open, drained, silent — silence starts now")
    else:
        usb, usb_lines = None, None
        print(f"{NODE_ID} answers over USB (firmware {version}); closing USB — silence starts now")
        node.ser.close()
    t_close = time.time()

    def usb_states() -> list[str]:
        if not usb_lines:
            return []
        out = [f"{s}" for l in usb_lines if '"silence_ms"' in l for s in STATE_RE.findall(l)]
        usb_lines.clear()
        return out

    base = serial.Serial()
    base.port, base.baudrate, base.timeout = args.base, 115200, 0.05
    base.dtr = base.rts = False
    base.open()
    base.dtr = base.rts = False
    time.sleep(0.3)
    base.reset_input_buffer()

    print(f"step 2: listening {args.silence:.0f}s for the offline edge ...")
    lines2, _ = drain(base, args.silence, also=usb, also_lines=usb_lines)
    st2 = link_states(lines2)
    usb2 = usb_states()
    offline_seen = any(s == "offline" for _, s in st2)
    print(f"  link_state via mesh: {st2} → offline {'seen' if offline_seen else 'NOT seen'}"
          + (f"; via USB: {usb2}" if args.watch_usb else ""))

    print("step 3: one mesh command; expecting link_state online")
    lines3, reply = probe(base, 0, args.online_within, also=usb, also_lines=usb_lines)
    st3 = link_states(lines3)
    usb3 = usb_states()
    online_seen = any(s == "online" for _, s in st3)
    print(f"  reply {'received' if reply else 'missing'}; link_state via mesh: {st3} → online "
          f"{'seen' if online_seen else 'NOT seen'}" + (f"; via USB: {usb3}" if args.watch_usb else ""))

    print(f"step 4: a command every {args.period:.0f}s for {args.hold:.0f}s; expecting no offline")
    t0 = time.time()
    i = 1
    st4: list[tuple[float, str]] = []
    lines4: list[str] = []
    replies = 0
    while time.time() - t0 < args.hold:
        ls, r = probe(base, i, args.period, also=usb, also_lines=usb_lines)
        lines4.extend(ls)
        replies += 1 if r else 0
        st4.extend((round(t0 - t_close + t, 1), s) for t, s in link_states(ls))
        i += 1
    usb4 = usb_states()
    offline_again = any(s == "offline" for _, s in st4)
    print(f"  {replies}/{i-1} replies; link_state via mesh: {st4} → offline "
          f"{'RECURRED' if offline_again else 'did not recur'}" + (f"; via USB: {usb4}" if args.watch_usb else ""))

    passed = offline_seen and online_seen and not offline_again
    print("PASS" if passed else "FAIL")

    stamp = time.strftime("%Y%m%d-%H%M%S")
    out = ROOT / "results" / f"bench_link_contact-{stamp}.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps({
        "label": args.label, "stamp": stamp, "firmware": version, "passed": passed,
        "step2_offline_seen": offline_seen, "step2_states": st2,
        "step3_online_seen": online_seen, "step3_reply": bool(reply), "step3_states": st3,
        "step4_offline_recurred": offline_again, "step4_replies": replies, "step4_sent": i - 1,
        "step4_states": st4,
        "watch_usb": args.watch_usb, "usb_states": {"step2": usb2, "step3": usb3, "step4": usb4},
        "lines": [l[:200] for l in lines2 + lines3 + lines4 if "SPINE" in l],
    }, indent=2))
    print(f"record -> {out}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
