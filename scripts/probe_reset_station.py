#!/usr/bin/env python3
"""probe_reset_station.py — reboot one Heltec through its CP2102 auto-reset circuit.

    python scripts/probe_reset_station.py COM3

Asserts RTS with DTR low for 200 ms (the esptool sequence; a DTR pulse alone
does nothing on this board), releases, and prints the boot banner for 3 s so
you can see the firmware, the root fingerprint, and the resumed counter.
"""

import sys
import time

import serial


def main() -> int:
    port = sys.argv[1] if len(sys.argv) > 1 else "COM3"
    s = serial.Serial()
    s.port, s.baudrate, s.timeout = port, 115200, 0.1
    s.dtr = False
    s.rts = False
    s.open()
    s.dtr = False
    s.rts = True
    time.sleep(0.2)
    s.rts = False
    t0 = time.time()
    while time.time() - t0 < 3.0:
        line = s.readline().decode(errors="replace").strip()
        if line and ("resumed" in line or "fingerprint" in line or "rst:" in line or "TX power" in line):
            print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
