"""Send a few descends; list every frame gw-40 transmits (seq, kind, bytes) next to
every src=40 frame the base receives, so the missing ones name themselves."""
import json, re, sys, time
import serial

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def quiet_open(port):
    s = serial.Serial()
    s.port, s.baudrate, s.timeout = port, 115200, 0.02
    s.dtr = s.rts = False
    s.open()
    s.dtr = s.rts = False
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
for p in (gw.port, base.port):
    p.reset_input_buffer()

tx = {}   # gw-40 seq -> (t, kind, bytes, snippet)
rx = {}   # seq -> (t, rssi)
t0 = time.time()
nxt, i = 1.0, 0
while time.time() - t0 < 40:
    now = time.time() - t0
    if now >= nxt and i < 4:
        cmd = {"id": f"d{i}", "to": "obc-esp32-s3-001", "cmd": "descend", "args": {"m": [[3, [1.0, 0.0][i % 2]]]}}
        base.port.write((json.dumps(cmd, separators=(",", ":")) + "\n").encode())
        print(f"{now:5.1f} sent d{i}")
        i += 1
        nxt += 8
    for l in gw.read():
        m = re.search(r"SPINE ► \((\w+)\) seq=(\d+)(?: \((\d+) B\))?(?: (.*))?", l)
        if m:
            tx[int(m.group(2))] = (round(now, 1), m.group(1), m.group(3) or "", (m.group(4) or "")[:50])
    for l in base.read():
        m = re.search(r"SPINE ◄ src=40 seq=(\d+) rssi=(-?\d+)", l)
        if m:
            rx[int(m.group(1))] = (round(now, 1), m.group(2))

print("\n gw-40 TX seq   t     kind       bytes  base RX?   payload")
for seq in sorted(tx):
    t, kind, nbytes, snip = tx[seq]
    got = f"yes {rx[seq][1]} dBm" if seq in rx else "NO"
    print(f"  {seq:4d}  {t:5.1f}  {kind:9s}  {nbytes:>4s}  {got:12s} {snip}")
missing = [s for s in tx if s not in rx]
print(f"\ngw-40 transmitted {len(tx)}, base received {len(tx) - len(missing)}; missing seqs: {missing}")
