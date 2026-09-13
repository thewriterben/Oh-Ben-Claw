"""One-off: which command line lengths does the node answer? Pads a harmless
command with an ignored arg and reports reply/no-reply per length."""
import json, sys, time
import serial

port = sys.argv[1] if len(sys.argv) > 1 else "COM6"
s = serial.Serial(port, 115200, timeout=0.5)
time.sleep(0.3)
s.reset_input_buffer()

def try_len(target: int, gap: float) -> bool:
    rid = f"L{target}"
    base = json.dumps({"id": rid, "cmd": "gpio_read", "args": {"pin": 0, "pad": ""}})
    pad = "A" * max(0, target - len(base))
    line = json.dumps({"id": rid, "cmd": "gpio_read", "args": {"pin": 0, "pad": pad}})
    s.reset_input_buffer()
    s.write((line + "\n").encode())
    t0 = time.time()
    while time.time() - t0 < 2.5:
        l = s.readline().decode(errors="replace").strip()
        if l.startswith("{") and f'"id":"{rid}"' in l:
            return True
    time.sleep(gap)
    return False

for target in [120, 200, 240, 250, 256, 260, 270, 290, 300, 310, 320, 340, 380, 450, 500]:
    ok = try_len(target, 0.2)
    print(f"{target:4d} bytes -> {'reply' if ok else 'NO REPLY'}")
# Does a dropped line eat the next one?
print("after a 340-byte line, a short one:", "reply" if (try_len(340, 0.0) or True) and try_len(120, 0.2) else "NO REPLY")
s.close()
