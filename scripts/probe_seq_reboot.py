"""SPINE-REPLAY.md §6 steps 1–3 on a Heltec: the seq counter never repeats
across reboots and skips at most RESERVE. A reset via the CP210x DTR line is an
*unclean* reboot (no orderly shutdown), so one procedure covers steps 1 and 2.

    python scripts/probe_seq_reboot.py COM3 [reboots]

Each cycle: listen quietly (DTR low) for a keepalive and note its seq and the
count the boot banner reported; open the port with DTR (reset); read the new
boot's "seq counter resumed at N" line and its first transmitted seq. Records
results/seq_reboot-<stamp>.json.
"""
import json, pathlib, re, sys, time
import serial

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
ANSI = re.compile(r"\x1b\[[0-9;]*m")
ROOT = pathlib.Path(__file__).resolve().parent.parent
RESERVE = 32


def open_port(port, reset: bool):
    s = serial.Serial()
    s.port, s.baudrate, s.timeout = port, 115200, 0.05
    if not reset:
        s.dtr = s.rts = False
    s.open()
    if not reset:
        s.dtr = s.rts = False
    return s


def read_lines(s, seconds, stop=None):
    buf, out, t0 = b"", [], time.time()
    while time.time() - t0 < seconds:
        chunk = s.read(4096)
        if not chunk:
            continue
        buf += chunk
        *whole, buf = buf.split(b"\n")
        for w in whole:
            l = ANSI.sub("", w.decode(errors="replace")).rstrip()
            if l:
                out.append(l)
                if stop and stop(l):
                    return out
    return out


port = sys.argv[1] if len(sys.argv) > 1 else "COM3"
cycles = int(sys.argv[2]) if len(sys.argv) > 2 else 5

# One open with DTR resets the board; so, it turned out, does closing and
# reopening it — the first draft of this probe counted "cycles" and saw a gap
# of two reserves per cycle. Boots are counted by their banner instead: the
# record is a list of boots, each with the count it resumed at and every seq
# it sent before the next banner. The judgement is per boot.
boots = []          # {"resumed": int, "seqs": [..], "disabled": bool}
all_lines = []
for i in range(cycles):
    s = open_port(port, reset=True)
    all_lines += read_lines(s, 6)
    s.close()
    time.sleep(0.5)
for l in all_lines:
    if m := re.search(r"seq counter resumed at (\d+)", l):
        boots.append({"resumed": int(m.group(1)), "seqs": [], "disabled": False})
    elif boots and (m := re.search(r"SPINE ► \(\w+\) seq=(\d+)", l)):
        boots[-1]["seqs"].append(int(m.group(1)))
    elif boots and "TRANSMIT DISABLED" in l:
        boots[-1]["disabled"] = True

ok = True
for a, b in zip(boots, boots[1:]):
    last_count = a["resumed"] + len(a["seqs"])       # counts are consecutive within a boot
    gap = b["resumed"] - last_count                  # numbers skipped by the reboot
    b["gap_after_previous_boot"] = gap
    b["first_seq_is_count_low_byte"] = (not b["seqs"]) or b["seqs"][0] == (b["resumed"] + 1) % 256
    if gap < 0 or gap > RESERVE or not b["first_seq_is_count_low_byte"]:
        ok = False
    print(f"boot {a['resumed']:5d} sent {len(a['seqs']):2d} → reboot → resumed {b['resumed']:5d}  "
          f"(skipped {gap:2d}, first seq {b['seqs'][:1]})")
counts = [b["resumed"] for b in boots]
monotone = all(y > x for x, y in zip(counts, counts[1:]))
print(f"\n{len(boots)} boots observed; resumed counts {counts}; strictly increasing: {monotone}; "
      f"every gap in [0, {RESERVE}] with first seq = count low byte: {ok}")
stamp = time.strftime("%Y%m%d-%H%M%S")
out = ROOT / "results" / f"seq_reboot-{stamp}.json"
out.write_text(json.dumps({"port": port, "reserve": RESERVE, "boots": boots,
                           "strictly_increasing": monotone, "gaps_within_reserve": ok}, indent=2))
print("record ->", out)
