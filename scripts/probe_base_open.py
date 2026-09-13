"""Does opening the base's CP210x port reboot it? Open with DTR/RTS held low and
watch for a boot banner vs. a continuing seq counter."""
import re, sys, time
import serial

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
port = sys.argv[1] if len(sys.argv) > 1 else "COM3"
mode = sys.argv[2] if len(sys.argv) > 2 else "quiet"

s = serial.Serial()
s.port, s.baudrate, s.timeout = port, 115200, 0.2
if mode == "quiet":
    s.dtr = False
    s.rts = False
s.open()
if mode == "quiet":
    s.dtr = False
    s.rts = False
t0 = time.time()
booted = False
seqs = []
while time.time() - t0 < 12:
    l = s.readline().decode(errors="replace").rstrip()
    if not l:
        continue
    if "ESP-ROM" in l or "spine gateway" in l or "cpu_start" in l:
        booted = True
        print(f"{time.time()-t0:5.1f} BOOT: {l[:100]}")
    m = re.search(r"SPINE ► \(keepalive\) seq=(\d+)", l)
    if m:
        seqs.append(int(m.group(1)))
        print(f"{time.time()-t0:5.1f} base keepalive seq {m.group(1)}")
print(f"mode={mode}: rebooted={booted}, seqs={seqs}")
s.close()
