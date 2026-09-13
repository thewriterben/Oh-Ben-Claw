"""Are the two Heltecs talking over each other? Log every transmission on both
consoles for a window (no commands sent) and, for each gw-40 frame the base
missed, report how far the base's nearest own transmission was."""
import re, sys, time
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
W = float(sys.argv[3]) if len(sys.argv) > 3 else 60
for p in (gw.port, base.port):
    p.reset_input_buffer()

gw_tx, base_tx, base_rx, gw_rx = {}, [], {}, {}
t0 = time.time()
while time.time() - t0 < W:
    now = round(time.time() - t0, 2)
    for l in gw.read():
        m = re.search(r"SPINE ► \((\w+)\) seq=(\d+)", l)
        if m:
            gw_tx[int(m.group(2))] = (now, m.group(1))
        m = re.search(r"SPINE ◄ src=D8 seq=(\d+)", l)
        if m:
            gw_rx[int(m.group(1))] = now
    for l in base.read():
        m = re.search(r"SPINE ► \((\w+)\) seq=(\d+)", l)
        if m:
            base_tx.append((now, int(m.group(2)), m.group(1)))
        m = re.search(r"SPINE ◄ src=40 seq=(\d+)", l)
        if m:
            base_rx[int(m.group(1))] = now

print(f"window {W:.0f} s: gw-40 sent {len(gw_tx)}, base heard {len(base_rx)}; "
      f"base sent {len(base_tx)}, gw-40 heard {len(gw_rx)}")
print("\n gw-40 seq   t      kind       base?   nearest base TX (dt)")
for seq in sorted(gw_tx):
    t, kind = gw_tx[seq]
    near = min(base_tx, key=lambda b: abs(b[0] - t), default=None)
    dt = f"{near[0]-t:+.2f}s ({near[2]})" if near else "-"
    print(f"  {seq:4d}  {t:6.2f}  {kind:9s}  {'yes' if seq in base_rx else 'NO ':4s}  {dt}")
print("\n base seq   t      kind       gw-40?")
for t, seq, kind in base_tx:
    print(f"  {seq:4d}  {t:6.2f}  {kind:9s}  {'yes' if seq in gw_rx else 'NO'}")
