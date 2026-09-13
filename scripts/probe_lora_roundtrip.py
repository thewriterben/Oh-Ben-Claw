"""One-off: send N spaced descends on the base; watch gw-40 and base consoles for
where each one stops — heard by the bridge, answered by the node on UART (and
transmitted by the bridge), or back at the base. For a reply the base missed,
say what the base was doing at that moment (its own TX makes it deaf).

    python scripts/probe_lora_roundtrip.py <gw40 port> <base port> [N]
"""
import json, re, sys, time
import serial

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def quiet_open(port: str) -> serial.Serial:
    """Open a CP210x port without toggling DTR/RTS — a default open resets the Heltec."""
    s = serial.Serial()
    s.port, s.baudrate, s.timeout = port, 115200, 0.02
    s.dtr = False
    s.rts = False
    s.open()
    s.dtr = False
    s.rts = False
    return s


class Lines:
    def __init__(self, port):
        self.port, self.buf = port, b""

    def read(self):
        chunk = self.port.read(4096)
        if not chunk:
            return []
        self.buf += chunk
        *whole, self.buf = self.buf.split(b"\n")
        return [ANSI.sub("", w.decode(errors="replace")).rstrip() for w in whole if w.strip()]


gw = Lines(quiet_open(sys.argv[1] if len(sys.argv) > 1 else "COM5"))
base = Lines(quiet_open(sys.argv[2] if len(sys.argv) > 2 else "COM3"))
N = int(sys.argv[3]) if len(sys.argv) > 3 else 5
GAP = 7.0
for p in (gw.port, base.port):
    p.reset_input_buffer()

t0 = time.time()
nxt, i = 0.0, 0
heard = replied = back = 0
base_tx = []        # (t, what) — every base transmission, for the deafness check
reply_tx = {}       # rid -> t gw-40 transmitted the reply
while time.time() - t0 < N * GAP + 8:
    now = time.time() - t0
    if now >= nxt and i < N:
        rid, lvl = f"c{i}", [1.0, 0.0][i % 2]
        cmd = {"id": rid, "to": "obc-esp32-s3-001", "cmd": "descend", "args": {"m": [[3, lvl]]}}
        base.port.write((json.dumps(cmd, separators=(",", ":")) + "\n").encode())
        print(f"{now:5.1f} sent {rid} level {lvl}")
        i += 1
        nxt += GAP
    for l in gw.read():
        m = re.search(r"SPINE ◄ src=D8 seq=(\d+) rssi=(-?\d+).*?: (.*)", l)
        if m and "descend" in m.group(3):
            heard += 1
            print(f"{now:5.1f} gw40 HEARD cmd, rssi {m.group(2)}: {m.group(3)[:80]}")
        m = re.search(r"SPINE ► \(uart\) seq=(\d+) \((\d+) B\) (.*)", l)
        if m and '"ok"' in m.group(3):
            rid = re.search(r'"id":"([^"]+)"', m.group(3))
            rid = rid.group(1) if rid else "?"
            replied += 1
            reply_tx[rid] = now
            print(f"{now:5.1f} gw40 TX reply {rid} (seq {m.group(1)}, {m.group(2)} B)")
    for l in base.read():
        if "SPINE ►" in l or "SPINE ⇒" in l:
            what = "relay" if "⇒" in l else ("keepalive" if "keepalive" in l else "console")
            base_tx.append((now, what))
        if "SPINE ◄ src=40" in l and '"ok"' in l:
            back += 1
            rid = re.search(r'"id":"([^"]+)"', l)
            rssi = re.search(r"rssi=(-?\d+)", l)
            print(f"{now:5.1f} base GOT reply {rid.group(1) if rid else '?'} at {rssi.group(1) if rssi else '?'} dBm")
        if "CRC" in l or "error" in l.lower():
            print(f"{now:5.1f} base: {l[:140]}")

print(f"\nsent {N}, gw40 heard {heard}, node replied (gw40 transmitted) {replied}, base received {back}")
for rid, t in reply_tx.items():
    near = [(round(tb - t, 2), w) for tb, w in base_tx if abs(tb - t) < 0.6]
    print(f"  reply {rid} went out at {t:5.1f}s; base TX within ±0.6 s: {near or 'none'}")
